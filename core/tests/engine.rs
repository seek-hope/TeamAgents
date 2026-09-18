//! Control-level scenario tests.
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
    let ctl = Control::new(store, "s1");
    // session bootstrap ensures runtime rows exist
    for a in spec().agents {
        ctl.store.ensure_agent("s1", &a.id).unwrap();
    }
    ctl
}

fn action(id: &str, actor: &str, kind: ActionKind, payload: Json, run_id: Option<String>) -> TeamAction {
    TeamAction { action_id: id.into(), session_id: "s1".into(), actor_id: actor.into(), run_id, kind, payload }
}

fn user(id: &str, text: &str) -> TeamAction {
    action(id, "user", ActionKind::UserMessage, json!({"text": text}), None)
}

#[test]
fn t1_user_message_queues_leader_run() {
    let mut ctl = harness();
    let r = ctl.submit(&user("a1", "please produce the report")).unwrap();
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
    let r2 = ctl.submit(&user("a1", "please produce the report")).unwrap();
    assert!(r2.ok);
    assert_eq!(ctl.store.events("s1", 0, 100).unwrap().len(), 1);
}

#[test]
fn t1_assign_task_dispatches_to_worker() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    let a = action(
        "a2",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "b", "description": "write the report", "acceptance": "report.md exists"}),
        Some(leader_run.run_id.clone()),
    );
    let task_id = derived_task_id(&a);
    let r = ctl.submit(&a).unwrap();
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
    ctl.submit(&user("a1", "go")).unwrap();

    // unknown assignee
    let r = ctl
        .submit(&action("v1", "leader", ActionKind::AssignTask, json!({"assignee": "ghost", "description": "x"}), None))
        .unwrap();
    assert!(!r.ok && r.error.unwrap().contains("unknown assignee"));
    // b cannot delegate (no task channel from b)
    let r = ctl
        .submit(&action("v2", "b", ActionKind::AssignTask, json!({"assignee": "leader", "description": "x"}), None))
        .unwrap();
    assert!(!r.ok && r.error.unwrap().contains("not allowed to assign"));
    // codex member only via leader — leader CAN, so check the reverse guard via update: use non-leader path
    // empty description refused
    let r = ctl
        .submit(&action("v3", "leader", ActionKind::AssignTask, json!({"assignee": "b", "description": "  "}), None))
        .unwrap();
    assert!(!r.ok && r.error.unwrap().contains("must not be empty"));
    // user cannot submit member actions
    let r =
        ctl.submit(&action("v4", "user", ActionKind::SendMessage, json!({"target": "b", "text": "x"}), None)).unwrap();
    assert!(!r.ok && r.error.unwrap().contains("local user cannot"));
    // unknown actor
    let r =
        ctl.submit(&action("v5", "ghost", ActionKind::SendMessage, json!({"target": "b", "text": "x"}), None)).unwrap();
    assert!(!r.ok && r.error.unwrap().contains("not a team member"));
    // b cannot message cx (no channel)
    let r =
        ctl.submit(&action("v6", "b", ActionKind::SendMessage, json!({"target": "cx", "text": "x"}), None)).unwrap();
    assert!(!r.ok);
    // publish to space without write access
    let r = ctl
        .submit(&action("v7", "leader", ActionKind::PublishShared, json!({"space_id": "lib", "content": "x"}), None))
        .unwrap();
    assert!(!r.ok && r.error.unwrap().contains("no write access"));
    // signal_done only by leader
    let r = ctl.submit(&action("v8", "b", ActionKind::SignalDone, json!({}), Some("run_x".into()))).unwrap();
    assert!(!r.ok);
}

#[test]
fn shared_publish_and_read_flow() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let r = ctl
        .submit(&action("s2", "b", ActionKind::PublishShared, json!({"space_id": "lib", "content": "findings"}), None))
        .unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    let seq = r.result["sequence"].as_i64().unwrap();
    assert!(seq >= 1);

    let r = ctl.submit(&action("s3", "leader", ActionKind::ReadShared, json!({"space_id": "lib"}), None)).unwrap();
    assert!(r.ok);
    let entries = r.result["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["content"], "findings");
    // cursor advanced: second read returns nothing new
    let r = ctl.submit(&action("s4", "leader", ActionKind::ReadShared, json!({"space_id": "lib"}), None)).unwrap();
    assert_eq!(r.result["entries"].as_array().unwrap().len(), 0);
}

/// A run interrupted mid-command stays OUTCOME_UNKNOWN until someone decides;
/// cancelling it is that decision and must clear the completion blocker.
/// A BLOCKED task (usually an interrupted turn) must be closable by the Leader,
/// otherwise nobody can get the goal back on track without the user.
#[test]
fn blocked_tasks_are_recoverable_by_the_leader() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    // leader assigns a task to b, then b's turn is interrupted: core blocks it
    let leader_run = ctl
        .store
        .runs_for_session("s1", &[TurnStatus::Queued, TurnStatus::Running])
        .unwrap()
        .iter()
        .find(|r| r.agent_id == "leader")
        .map(|r| r.run_id.clone());
    let r = ctl
        .submit(&action(
            "t1",
            "leader",
            ActionKind::AssignTask,
            json!({"assignee": "b", "description": "fix it"}),
            leader_run.clone(),
        ))
        .unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    let task_id = r.result["task_id"].as_str().unwrap().to_string();
    ctl.store.compare_and_set_task(&task_id, "PENDING", TaskStatus::Blocked, None).unwrap();

    // the assignee cannot finish it any more
    let assignee_run = ctl
        .store
        .runs_for_session("s1", &[TurnStatus::Queued, TurnStatus::Running])
        .unwrap()
        .iter()
        .find(|r| r.agent_id == "b")
        .map(|r| r.run_id.clone());
    let error = ctl
        .submit(&action("c1", "b", ActionKind::CompleteTask, json!({"task_id": task_id}), assignee_run))
        .unwrap()
        .error
        .unwrap_or_default();
    assert!(error.contains("BLOCKED") || error.contains("cannot complete"), "{error}");

    // the Leader closes it and re-issues the work as a new task
    let closed = ctl
        .submit(&action("x1", "leader", ActionKind::CancelTask, json!({"task_id": task_id}), leader_run.clone()))
        .unwrap();
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    assert_eq!(ctl.store.get_task(&task_id).unwrap().unwrap().status, TaskStatus::Cancelled);
    let reassigned = ctl
        .submit(&action(
            "t2",
            "leader",
            ActionKind::AssignTask,
            json!({"assignee": "b", "description": "fix it (retry)"}),
            leader_run.clone(),
        ))
        .unwrap();
    assert!(reassigned.ok, "{}", reassigned.error.unwrap_or_default());
    assert_ne!(reassigned.result["task_id"], json!(task_id), "a retry is a new task, not a reopened one");
}

#[test]
fn acknowledging_an_unknown_run_unblocks_completion() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let run_id = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap()[0].run_id.clone();
    ctl.store.set_run_status(&run_id, TurnStatus::OutcomeUnknown).unwrap();

    // the resumed turn keeps its run id, so the leader signals done against the
    // very run whose outcome is unknown
    let signal =
        |id: &str| action(id, "leader", ActionKind::SignalDone, json!({"summary": "done"}), Some(run_id.clone()));

    // the unknown run blocks completion, and the receipt says how to clear it
    let r = ctl.submit(&signal("sd1")).unwrap();
    assert!(!r.ok);
    let blockers = r.result["blockers"].as_array().cloned().unwrap_or_default();
    let text = blockers.iter().filter_map(|b| b.as_str()).collect::<Vec<_>>().join(" | ");
    assert!(text.contains("outcome-unknown") && text.contains(&run_id), "{text}");
    assert!(text.contains("cancel_run <run_id>"), "the blocker says how to clear it: {text}");
    assert!(!text.contains(&format!("{run_id} of b:")), "the run id is never glued to another field: {text}");

    let r = ctl.submit(&action("ack1", "leader", ActionKind::CancelRun, json!({"run_id": run_id}), None)).unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    assert_eq!(r.result["status"], "acknowledged");
    assert_eq!(ctl.store.get_run(&run_id).unwrap().unwrap().status, TurnStatus::Cancelled);
    let r = ctl.submit(&signal("sd2")).unwrap();
    let text = r.result["blockers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|b| b.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(!text.contains("outcome-unknown"), "acknowledgement cleared the blocker: {text}");

    // an already-terminal run cannot be cancelled again
    let r = ctl.submit(&action("ack2", "leader", ActionKind::CancelRun, json!({"run_id": run_id}), None)).unwrap();
    assert!(!r.ok);
    let error = r.error.clone().unwrap_or_default();
    assert!(error.contains("already ended"), "{error}");
}

#[test]
fn refused_messages_and_spaces_name_the_valid_options() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();

    // b may only message leader: the refusal names who is actually reachable
    let r =
        ctl.submit(&action("m1", "b", ActionKind::SendMessage, json!({"target": "cx", "text": "hi"}), None)).unwrap();
    assert!(!r.ok);
    let error = r.error.unwrap_or_default();
    assert!(error.contains("no channel covers this direction"), "{error}");
    assert!(error.contains("reachable now") && error.contains("leader"), "{error}");

    // a wrong space id is corrected with the spaces this member can use
    let r = ctl.submit(&action("m2", "b", ActionKind::ReadShared, json!({"space_id": "nope"}), None)).unwrap();
    assert!(!r.ok);
    let error = r.error.unwrap_or_default();
    assert!(error.contains("available") && error.contains("lib"), "{error}");
}

#[test]
fn topology_patch_add_and_stale_reject() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let rev = ctl.store.current_revision("s1").unwrap();

    // propose by member
    let r = ctl.submit(&action(
        "p1",
        "b",
        ActionKind::ProposeTeamChange,
        json!({"operations": [{"op": "add_channel", "channel": {"source": "leader", "targets": ["cx"], "mode": "message"}}]}),
        None,
    )).unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    let patch_id = r.result["patch_id"].as_str().unwrap().to_string();

    // stale base rejected on apply path only after revision moves; apply now works (leader)
    let r = ctl
        .submit(&action("p2", "leader", ActionKind::ApplyTopologyPatch, json!({"patch_id": patch_id}), None))
        .unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    assert_eq!(ctl.store.current_revision("s1").unwrap(), rev + 1);
    let spec = ctl.store.load_team_spec("s1", None).unwrap();
    assert!(spec.can_send("leader", "cx"));

    // rejecting an already-applied patch fails
    let r = ctl
        .submit(&action("p3", "leader", ActionKind::ApplyTopologyPatch, json!({"patch_id": patch_id}), None))
        .unwrap();
    assert!(!r.ok);

    // a member-to-member channel is refused (D-33): members use shared spaces
    let r = ctl.submit(&action(
        "p3b",
        "leader",
        ActionKind::ApplyTopologyPatch,
        json!({"base_revision": ctl.store.current_revision("s1").unwrap(),
               "operations": [{"op": "add_channel", "channel": {"source": "b", "targets": ["cx"], "mode": "message"}}]}),
        None,
    )).unwrap();
    assert!(!r.ok);
    assert!(r.error.unwrap_or_default().contains("member-to-member"));

    // an inline patch without base_revision must name the revision to resend
    let revision = ctl.store.current_revision("s1").unwrap();
    let ops = json!({"operations": [{"op": "add_channel", "channel": {"source": "leader", "targets": ["b"], "mode": "message"}}]});
    let error = ctl
        .submit(&action("p4", "leader", ActionKind::ApplyTopologyPatch, ops.clone(), None))
        .unwrap()
        .error
        .unwrap_or_default();
    assert!(error.contains(&revision.to_string()), "the leader must be told the current revision: {error}");
    let mut with_revision = ops;
    with_revision["base_revision"] = json!(revision);
    let r = ctl.submit(&action("p5", "leader", ActionKind::ApplyTopologyPatch, with_revision, None)).unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
}

#[test]
fn cancel_task_without_run_cancels_immediately() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    let a = action(
        "c1",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "b", "description": "x"}),
        Some(leader_run.run_id),
    );
    let task_id = derived_task_id(&a);
    ctl.submit(&a).unwrap();
    // cancel the (not yet started) task as the user
    let r = ctl.submit(&action("c2", "user", ActionKind::CancelTask, json!({"task_id": task_id}), None)).unwrap();
    assert!(r.ok);
    let task = ctl.store.get_task(&task_id).unwrap().unwrap();
    assert!(matches!(task.status, TaskStatus::Cancelled));
}

fn running_task(task_id: &str, assignee: &str) -> Task {
    let mut t: Task = serde_json::from_value(json!({
        "task_id": task_id, "requester": "leader", "assignee": assignee, "description": "d"
    }))
    .unwrap();
    t.status = TaskStatus::Running;
    t
}

fn parked_run(run_id: &str, agent: &str, task_id: &str, status: TurnStatus, waiting_on: Vec<String>) -> TurnRun {
    TurnRun {
        run_id: run_id.into(),
        session_id: "s1".into(),
        task_id: Some(task_id.into()),
        goal_id: None,
        agent_id: agent.into(),
        config_revision: 0,
        topology_revision: 0,
        status,
        input_delivery_ids: vec![],
        context_ref: None,
        external_turn_id: None,
        cancel_requested: false,
        waiting_on,
        created_at: 1000.0,
        updated_at: 1000.0,
    }
}

#[test]
fn cancel_task_converges_run_parked_on_task_wait() {
    let mut ctl = harness();
    // b's turn is parked in WAITING_TASK on w-task; its own a-task is RUNNING
    ctl.store.insert_task("s1", &running_task("w-task", "leader")).unwrap();
    ctl.store.insert_task("s1", &running_task("a-task", "b")).unwrap();
    ctl.store.insert_run(&parked_run("run-b", "b", "a-task", TurnStatus::WaitingTask, vec!["w-task".into()])).unwrap();
    ctl.store.insert_run(&parked_run("run-l", "leader", "w-task", TurnStatus::Running, vec![])).unwrap();

    let r = ctl.submit(&action("cw1", "user", ActionKind::CancelTask, json!({"task_id": "a-task"}), None)).unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    // the parked run converges to CANCELLED instead of waiting for its wake
    assert_eq!(ctl.store.get_run("run-b").unwrap().unwrap().status, TurnStatus::Cancelled);
    assert!(matches!(ctl.store.get_task("a-task").unwrap().unwrap().status, TaskStatus::Cancelled));

    // w-task completing must not resurrect the cancelled run or task
    ctl.store.record_completion_request("run-l", "w-task", &[], "w done").unwrap();
    ctl.finalize_run("run-l", &completed(None), &[]).unwrap();
    ctl.schedule().unwrap();
    assert_eq!(
        ctl.store.get_run("run-b").unwrap().unwrap().status,
        TurnStatus::Cancelled,
        "cancelled run must not revive"
    );
    assert!(matches!(ctl.store.get_task("a-task").unwrap().unwrap().status, TaskStatus::Cancelled));
}

#[test]
fn rolled_back_transaction_drops_mid_turn_pushes() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    let a = action(
        "mp1",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "b", "description": "first"}),
        Some(leader_run.run_id.clone()),
    );
    ctl.submit(&a).unwrap();
    // b's turn is in flight: a fresh assignment becomes a mid-turn push
    let b_run = ctl
        .store
        .runs_for_session("s1", &[TurnStatus::Queued])
        .unwrap()
        .into_iter()
        .find(|r| r.agent_id == "b")
        .unwrap();
    ctl.begin_run(&b_run.run_id).unwrap();
    ctl.drain_mid_turn_pushes();

    // fail the tx after schedule pushed: record_action is the last write before commit
    ctl.store
        .conn
        .execute_batch(
            "CREATE TRIGGER boom_push BEFORE INSERT ON actions WHEN NEW.action_id='mp2'
             BEGIN SELECT RAISE(ABORT, 'boom_push'); END;",
        )
        .unwrap();
    let a = action(
        "mp2",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "b", "description": "second"}),
        Some(leader_run.run_id.clone()),
    );
    let err = ctl.submit(&a).unwrap_err();
    assert!(err.contains("boom_push"), "{err}");
    assert!(ctl.drain_mid_turn_pushes().is_empty(), "a rolled-back push must never drain");

    // the next clean submit rebuilds the push from persisted state
    ctl.store.conn.execute_batch("DROP TRIGGER boom_push").unwrap();
    let a = action(
        "mp3",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "b", "description": "second"}),
        Some(leader_run.run_id),
    );
    let r = ctl.submit(&a).unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    let pushes = ctl.drain_mid_turn_pushes();
    assert_eq!(pushes.len(), 1);
    assert_eq!(pushes[0].0, b_run.run_id);
}

#[test]
fn failed_transaction_keeps_prior_committed_pushes() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    let a = action(
        "kp1",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "b", "description": "first"}),
        Some(leader_run.run_id.clone()),
    );
    ctl.submit(&a).unwrap();
    let b_run = ctl
        .store
        .runs_for_session("s1", &[TurnStatus::Queued])
        .unwrap()
        .into_iter()
        .find(|r| r.agent_id == "b")
        .unwrap();
    ctl.begin_run(&b_run.run_id).unwrap();

    // tx#1 commits with a mid-turn push; drain is a separate RPC, so it stays queued
    let a = action(
        "kp2",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "b", "description": "second"}),
        Some(leader_run.run_id.clone()),
    );
    ctl.submit(&a).unwrap();

    // tx#2 pushes, then fails at the final write before commit
    ctl.store
        .conn
        .execute_batch(
            "CREATE TRIGGER boom_push BEFORE INSERT ON actions WHEN NEW.action_id='kp3'
             BEGIN SELECT RAISE(ABORT, 'boom_push'); END;",
        )
        .unwrap();
    let a = action(
        "kp3",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "b", "description": "third"}),
        Some(leader_run.run_id),
    );
    let err = ctl.submit(&a).unwrap_err();
    assert!(err.contains("boom_push"), "{err}");

    // tx#1's push survives; tx#2's own push is dropped with its rollback
    let pushes = ctl.drain_mid_turn_pushes();
    assert_eq!(pushes.len(), 1, "{pushes:?}");
    assert_eq!(pushes[0].0, b_run.run_id);
    let body = serde_json::to_string(&pushes[0].1).unwrap();
    assert!(body.contains("second") && !body.contains("third"), "{body}");
}

#[test]
fn complete_task_requires_an_active_run() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    let a = action(
        "ct1",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "b", "description": "x"}),
        Some(leader_run.run_id),
    );
    let task_id = derived_task_id(&a);
    ctl.submit(&a).unwrap();

    // completion without a run_id is refused; no orphan completion is recorded
    let r = ctl
        .submit(&action("ct2", "b", ActionKind::CompleteTask, json!({"task_id": task_id, "summary": "s"}), None))
        .unwrap();
    assert!(!r.ok);
    assert!(r.error.unwrap().contains("requires an active run"));
    let task = ctl.store.get_task(&task_id).unwrap().unwrap();
    assert!(matches!(task.status, TaskStatus::Pending));
}

#[test]
fn cancel_task_cancels_queued_run() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    let a = action(
        "cq1",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "b", "description": "x"}),
        Some(leader_run.run_id.clone()),
    );
    let task_id = derived_task_id(&a);
    ctl.submit(&a).unwrap();
    let b_run = ctl
        .store
        .runs_for_session("s1", &[TurnStatus::Queued])
        .unwrap()
        .into_iter()
        .find(|r| r.agent_id == "b")
        .unwrap();

    let r = ctl.submit(&action("cq2", "user", ActionKind::CancelTask, json!({"task_id": task_id}), None)).unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    // the queued run is cancelled outright: the engine must never begin it
    assert_eq!(ctl.store.get_run(&b_run.run_id).unwrap().unwrap().status, TurnStatus::Cancelled);
    assert!(matches!(ctl.store.get_task(&task_id).unwrap().unwrap().status, TaskStatus::Cancelled));
    // the unrelated leader run is untouched
    assert_eq!(ctl.store.get_run(&leader_run.run_id).unwrap().unwrap().status, TurnStatus::Queued);
}

#[test]
fn waiting_boundary_patch_can_be_rejected_and_releases_draining() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    let a = action(
        "wb1",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "b", "description": "x"}),
        Some(leader_run.run_id),
    );
    ctl.submit(&a).unwrap(); // b now has a QUEUED (live) run

    let r = ctl
        .submit(&action(
            "wb2",
            "b",
            ActionKind::ProposeTeamChange,
            json!({"operations": [{"op": "update_agent", "agent_id": "b", "changes": {"name": "B2"}}]}),
            None,
        ))
        .unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    let patch_id = r.result["patch_id"].as_str().unwrap().to_string();

    let r = ctl
        .submit(&action("wb3", "leader", ActionKind::ApplyTopologyPatch, json!({"patch_id": patch_id}), None))
        .unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    assert_eq!(r.result["status"], json!("WAITING_BOUNDARY"));
    assert_eq!(ctl.store.agent_status("s1", "b").unwrap(), Some(AgentStatus::Draining));

    // the Leader can reject a boundary-parked patch; parked members leave Draining
    let r = ctl
        .submit(&action(
            "wb4",
            "leader",
            ActionKind::ApplyTopologyPatch,
            json!({"patch_id": patch_id, "reject": true}),
            None,
        ))
        .unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    assert_eq!(r.result["status"], json!("REJECTED"));
    assert_eq!(ctl.store.get_patch(&patch_id).unwrap().unwrap().status, PatchStatus::Rejected);
    assert_eq!(ctl.store.agent_status("s1", "b").unwrap(), Some(AgentStatus::Idle));
}

#[test]
fn approval_parked_run_does_not_block_boundary() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    let a = action(
        "ap1",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "b", "description": "x"}),
        Some(leader_run.run_id),
    );
    ctl.submit(&a).unwrap();
    let b_run = ctl
        .store
        .runs_for_session("s1", &[TurnStatus::Queued])
        .unwrap()
        .into_iter()
        .find(|r| r.agent_id == "b")
        .unwrap();
    // in-process run parked on an approval: its thread exited, external_turn_id is None
    ctl.store.set_run_status(&b_run.run_id, TurnStatus::WaitingApproval).unwrap();

    let r = ctl
        .submit(&action(
            "ap2",
            "b",
            ActionKind::ProposeTeamChange,
            json!({"operations": [{"op": "update_agent", "agent_id": "b", "changes": {"name": "B2"}}]}),
            None,
        ))
        .unwrap();
    let patch_id = r.result["patch_id"].as_str().unwrap().to_string();
    let r = ctl
        .submit(&action("ap3", "leader", ActionKind::ApplyTopologyPatch, json!({"patch_id": patch_id}), None))
        .unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    // applies immediately instead of deadlocking behind the parked approval
    assert_eq!(r.result["status"], json!("APPLIED"));
    assert_eq!(ctl.store.get_patch(&patch_id).unwrap().unwrap().status, PatchStatus::Applied);
}

#[test]
fn task_wait_parked_run_does_not_block_boundary() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    let a = action(
        "tw1",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "b", "description": "x"}),
        Some(leader_run.run_id.clone()),
    );
    ctl.submit(&a).unwrap();
    // a long-running task for b to wait on (empty waiting_on would be woken by schedule)
    let a = action(
        "tw0",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "leader", "description": "slow"}),
        Some(leader_run.run_id),
    );
    ctl.submit(&a).unwrap();
    let b_run = ctl
        .store
        .runs_for_session("s1", &[TurnStatus::Queued])
        .unwrap()
        .into_iter()
        .find(|r| r.agent_id == "b")
        .unwrap();
    let w_task =
        ctl.store.tasks_for_session("s1", &["PENDING"]).unwrap().into_iter().find(|t| t.assignee == "leader").unwrap();
    // in-process run parked on a task wait: its thread exited, external_turn_id is None
    ctl.store.set_run_status(&b_run.run_id, TurnStatus::WaitingTask).unwrap();
    ctl.store.deliver_wait_registration(&b_run.run_id, &[w_task.task_id]).unwrap();

    let r = ctl
        .submit(&action(
            "tw2",
            "b",
            ActionKind::ProposeTeamChange,
            json!({"operations": [{"op": "update_agent", "agent_id": "b", "changes": {"name": "B2"}}]}),
            None,
        ))
        .unwrap();
    let patch_id = r.result["patch_id"].as_str().unwrap().to_string();
    let r = ctl
        .submit(&action("tw3", "leader", ActionKind::ApplyTopologyPatch, json!({"patch_id": patch_id}), None))
        .unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    // applies immediately instead of parking behind the awaited task
    assert_eq!(r.result["status"], json!("APPLIED"));
    assert_eq!(ctl.store.get_patch(&patch_id).unwrap().unwrap().status, PatchStatus::Applied);
}

#[test]
fn signal_done_blocked_by_unfinished_work() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    // mark the leader run RUNNING so it can signal
    ctl.store.set_run_status(&leader_run.run_id, TurnStatus::Running).unwrap();
    let a = action(
        "d1",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "b", "description": "x"}),
        Some(leader_run.run_id.clone()),
    );
    ctl.submit(&a).unwrap();
    let r = ctl
        .submit(&action(
            "d2",
            "leader",
            ActionKind::SignalDone,
            json!({"summary": "done"}),
            Some(leader_run.run_id.clone()),
        ))
        .unwrap();
    assert!(!r.ok);
    assert!(r.error.unwrap().contains("not yet complete"));
    let blockers = r.result["blockers"].as_array().unwrap();
    assert!(blockers.iter().any(|b| b.as_str().unwrap().contains("unfinished tasks")));
}

// -- finalize_run ---------------------------------------------------------------

use teamagents_core::control::TurnOutcome;

fn completed(reply: Option<&str>) -> TurnOutcome {
    TurnOutcome { status: TurnStatus::Completed, error: None, note: None, reply_text: reply.map(str::to_string) }
}

/// 竞态（CI 上偶发，见 engine/tests/chat_e2e.rs 的审批用例）：批准请求先落库、
/// 用户/自动化马上拍板，回合随后才报告"停在等批准"。此时已经没有 PENDING 行，
/// 若照旧把回合停在 WAITING_APPROVAL，就再也没人来唤醒它（pending 0 + WAITING_APPROVAL 卡死）。
#[test]
fn run_parked_after_its_approval_was_decided_is_woken_instead_of_stuck() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    ctl.begin_run(&run.run_id).unwrap();

    // 网关先落 PENDING，用户立刻拍板
    ctl.store
        .insert_approval(&ApprovalRequest {
            approval_id: "appr-race".into(),
            session_id: "s1".into(),
            agent_id: run.agent_id.clone(),
            run_id: run.run_id.clone(),
            tool_call_id: "call-1".into(),
            operation_hash: "h-race".into(),
            requested_scope: json!({"tool": "shell"}),
            policy_revision: 1,
            status: ApprovalStatus::Pending,
            created_at: 1.0,
            decided_at: None,
        })
        .unwrap();
    let decided = ctl
        .submit(&action(
            "ad1",
            "user",
            ActionKind::ApprovalDecision,
            json!({"approval_id": "appr-race", "decision": "once"}),
            None,
        ))
        .unwrap();
    assert!(decided.ok, "{}", decided.error.unwrap_or_default());

    // 决定已经落库，回合才走到"停在等批准"
    ctl.finalize_run(
        &run.run_id,
        &TurnOutcome { status: TurnStatus::WaitingApproval, error: None, note: None, reply_text: None },
        &[],
    )
    .unwrap();

    let after = ctl.store.get_run(&run.run_id).unwrap().unwrap();
    assert_eq!(after.status, TurnStatus::Running, "已决定的批准不能把回合留在 WAITING_APPROVAL 里等一个不会到来的决定");
}

#[test]
fn late_completion_does_not_resurrect_a_cancelled_task_or_run() {
    let mut ctl = harness();
    ctl.store.insert_task("s1", &running_task("child", "leader")).unwrap();
    ctl.store.insert_task("s1", &running_task("work", "b")).unwrap();
    ctl.store.insert_run(&parked_run("worker", "b", "work", TurnStatus::WaitingTask, vec!["child".into()])).unwrap();
    ctl.store.record_completion_request("worker", "work", &["result.txt".into()], "ready").unwrap();

    assert!(
        ctl.submit(&action("cancel", "user", ActionKind::CancelTask, json!({"task_id": "work"}), None)).unwrap().ok
    );
    assert_eq!(ctl.store.get_run("worker").unwrap().unwrap().status, TurnStatus::Cancelled);
    let before = ctl.store.events("s1", 0, 100).unwrap();
    ctl.finalize_run("worker", &completed(Some("late result")), &[]).unwrap();

    assert_eq!(ctl.store.get_run("worker").unwrap().unwrap().status, TurnStatus::Cancelled);
    assert_eq!(ctl.store.get_task("work").unwrap().unwrap().status, TaskStatus::Cancelled);
    assert_eq!(
        ctl.store.events("s1", 0, 100).unwrap(),
        before,
        "a stale callback must not emit a second terminal event"
    );
}

#[test]
fn cancelled_queued_run_cannot_begin() {
    let mut ctl = harness();
    ctl.store.insert_task("s1", &running_task("work", "b")).unwrap();
    ctl.store.insert_run(&parked_run("worker", "b", "work", TurnStatus::Queued, vec![])).unwrap();
    assert!(
        ctl.submit(&action("cancel", "user", ActionKind::CancelTask, json!({"task_id": "work"}), None)).unwrap().ok
    );
    let before = ctl.store.events("s1", 0, 100).unwrap();
    assert!(ctl.begin_run("worker").is_err(), "a stale scheduler snapshot must not start a cancelled run");
    assert_eq!(ctl.store.events("s1", 0, 100).unwrap(), before);
    assert_eq!(ctl.store.get_run("worker").unwrap().unwrap().status, TurnStatus::Cancelled);
}

#[test]
fn finalize_commits_task_and_wakes_waiter() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);

    // leader assigns to b; b's run is queued by schedule
    let a = action(
        "f1",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "b", "description": "report"}),
        Some(leader_run.run_id.clone()),
    );
    let task_id = derived_task_id(&a);
    ctl.submit(&a).unwrap();

    // leader run must be RUNNING before wait_for_tasks can park it
    ctl.begin_run(&leader_run.run_id).unwrap();
    // leader waits on the task: its run parks in WAITING_TASK
    let r = ctl
        .submit(&action(
            "f2",
            "leader",
            ActionKind::WaitForTasks,
            json!({"task_ids": [task_id]}),
            Some(leader_run.run_id.clone()),
        ))
        .unwrap();
    assert!(r.ok && r.result["waiting"] == json!(true));

    // b begins + completes with a completion request
    let b_run = ctl
        .store
        .runs_for_session("s1", &[TurnStatus::Queued])
        .unwrap()
        .into_iter()
        .find(|r| r.agent_id == "b")
        .unwrap();
    ctl.begin_run(&b_run.run_id).unwrap();
    let task = ctl.store.get_task(&task_id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running); // begin_run started the task

    ctl.submit(&action(
        "f3",
        "b",
        ActionKind::CompleteTask,
        json!({"task_id": task_id, "result_refs": ["artifacts/report.md"], "summary": "wrote it"}),
        Some(b_run.run_id.clone()),
    ))
    .unwrap();
    ctl.finalize_run(&b_run.run_id, &completed(Some("done")), &[b_run.input_delivery_ids[0]]).unwrap();

    let task = ctl.store.get_task(&task_id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Succeeded);
    assert_eq!(task.result_refs, vec!["artifacts/report.md"]);

    // waiter (leader) run resumed and received the completion event delivery
    let leader_run = ctl.store.get_run(&leader_run.run_id).unwrap().unwrap();
    assert_eq!(leader_run.status, TurnStatus::Running);

    let events = ctl.store.events("s1", 0, 100).unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"task_started"));
    assert!(kinds.contains(&"task_completed"));
    assert!(kinds.contains(&"run_completed"));

    // b's delivery acked: nothing pending for b
    assert_eq!(ctl.store.pending_deliveries("s1", "b").unwrap().len(), 0);
}

#[test]
fn finalize_goal_done_after_signal_done() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "say hello")).unwrap();
    let run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    ctl.begin_run(&run.run_id).unwrap();
    let r = ctl
        .submit(&action(
            "g1",
            "leader",
            ActionKind::SignalDone,
            json!({"summary": "answered"}),
            Some(run.run_id.clone()),
        ))
        .unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    ctl.finalize_run(&run.run_id, &completed(Some("hello!")), &[]).unwrap();

    let session = ctl.store.get_session("s1").unwrap().unwrap();
    assert_eq!(session["goal_state"], json!("done"));
    let events = ctl.store.events("s1", 0, 100).unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"goal_done"));
    assert!(kinds.contains(&"leader_reply"));

    // a fresh user message starts a new goal
    let r = ctl.submit(&user("a2", "second question")).unwrap();
    let goal2 = r.result["goal_id"].as_str().unwrap().to_string();
    assert_ne!(goal2, session["goal_id"].as_str().unwrap());
}

#[test]
fn finalize_rechecks_work_added_after_signal_done() {
    let mut ctl = harness();
    ctl.submit(&user("start", "finish the work")).unwrap();
    let run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    ctl.begin_run(&run.run_id).unwrap();
    assert!(
        ctl.submit(&action(
            "done",
            "leader",
            ActionKind::SignalDone,
            json!({"summary": "ready"}),
            Some(run.run_id.clone())
        ))
        .unwrap()
        .ok
    );
    let assigned = ctl
        .submit(&action(
            "extra",
            "leader",
            ActionKind::AssignTask,
            json!({"assignee": "b", "description": "one more check"}),
            Some(run.run_id.clone()),
        ))
        .unwrap();
    assert!(assigned.ok);
    ctl.finalize_run(&run.run_id, &completed(Some("ready")), &run.input_delivery_ids).unwrap();
    assert_eq!(ctl.store.get_run(&run.run_id).unwrap().unwrap().status, TurnStatus::Completed);
    assert_eq!(ctl.store.get_session("s1").unwrap().unwrap()["goal_state"], "active");
    assert!(ctl.store.events("s1", 0, 100).unwrap().iter().all(|e| e["kind"] != "goal_done"));
    assert_eq!(
        ctl.store.get_task(assigned.result["task_id"].as_str().unwrap()).unwrap().unwrap().status,
        TaskStatus::Pending
    );
}

#[test]
fn new_user_input_invalidates_an_earlier_goal_completion_request() {
    for (delivered, renewed) in [(false, false), (true, false), (true, true)] {
        let mut ctl = harness();
        ctl.submit(&user("start", "first request")).unwrap();
        let run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
        ctl.begin_run(&run.run_id).unwrap();
        assert!(
            ctl.submit(&action(
                "done",
                "leader",
                ActionKind::SignalDone,
                json!({"summary": "ready"}),
                Some(run.run_id.clone())
            ))
            .unwrap()
            .ok
        );
        assert!(ctl.submit(&user("supplement", "also check the new requirement")).unwrap().ok);
        if renewed {
            assert!(
                ctl.submit(&action(
                    "done-again",
                    "leader",
                    ActionKind::SignalDone,
                    json!({"summary": "new requirement handled"}),
                    Some(run.run_id.clone())
                ))
                .unwrap()
                .ok
            );
        }
        let ack_ids = if delivered {
            ctl.store.get_run(&run.run_id).unwrap().unwrap().input_delivery_ids
        } else {
            run.input_delivery_ids.clone()
        };
        ctl.finalize_run(&run.run_id, &completed(Some("old answer")), &ack_ids).unwrap();
        assert_eq!(
            ctl.store.get_session("s1").unwrap().unwrap()["goal_state"],
            if renewed { "done" } else { "active" },
            "delivered={delivered}, renewed={renewed}"
        );
        assert_eq!(ctl.store.events("s1", 0, 100).unwrap().iter().any(|e| e["kind"] == "goal_done"), renewed);
        if !delivered {
            assert_eq!(
                ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().len(),
                1,
                "new input still wakes the leader"
            );
        }
    }
}

#[test]
fn stale_goal_completion_cannot_overwrite_a_new_goal() {
    let mut ctl = harness();
    ctl.submit(&user("start", "first request")).unwrap();
    let run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    ctl.begin_run(&run.run_id).unwrap();
    ctl.store.set_goal_state("s1", run.goal_id.as_deref().unwrap(), "done").unwrap();
    let next = ctl.submit(&user("next", "new goal")).unwrap();
    let next_goal = next.result["goal_id"].as_str().unwrap();
    assert_ne!(Some(next_goal), run.goal_id.as_deref());
    // Model a stale persisted request being reconciled after a newer goal starts.
    ctl.store.record_completion_request(&run.run_id, "", &[], "old goal").unwrap();
    ctl.finalize_run(&run.run_id, &completed(Some("old answer")), &run.input_delivery_ids).unwrap();
    let session = ctl.store.get_session("s1").unwrap().unwrap();
    assert_eq!(session["goal_id"], next_goal);
    assert_eq!(session["goal_state"], "active");
    assert!(ctl.store.events("s1", 0, 100).unwrap().iter().all(|e| e["kind"] != "goal_done"));
}

#[test]
fn confirmed_recovery_can_commit_a_pending_goal_completion() {
    let mut ctl = harness();
    ctl.submit(&user("start", "finish the work")).unwrap();
    let run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    ctl.begin_run(&run.run_id).unwrap();
    assert!(
        ctl.submit(&action(
            "done",
            "leader",
            ActionKind::SignalDone,
            json!({"summary": "ready"}),
            Some(run.run_id.clone())
        ))
        .unwrap()
        .ok
    );
    ctl.store.set_run_status(&run.run_id, TurnStatus::OutcomeUnknown).unwrap();
    ctl.finalize_run(&run.run_id, &completed(Some("confirmed answer")), &run.input_delivery_ids).unwrap();
    assert_eq!(ctl.store.get_session("s1").unwrap().unwrap()["goal_state"], "done");
}

// -- hardening: store errors, ledger, lifecycle events ---------------------------

use teamagents_core::control::{payload_hash, EventDraft};

fn plain_action(id: &str, payload: Json) -> TeamAction {
    action(id, "user", ActionKind::UserMessage, payload, None)
}

#[test]
fn emit_reports_store_errors_instead_of_swallowing_them() {
    let path = std::env::temp_dir().join(format!("ta_core_{}.db", new_id("lock")));
    let store = Store::open(&path).unwrap();
    store.create_session("s1", "/tmp", "approved_scope").unwrap();
    store.save_team_spec("s1", &spec()).unwrap();
    for a in spec().agents {
        store.ensure_agent("s1", &a.id).unwrap();
    }
    // fail fast instead of waiting out the production busy_timeout
    store.conn.busy_timeout(std::time::Duration::from_millis(50)).unwrap();
    let mut ctl = Control::new(store, "s1");

    // another writer holds the write lock for the whole call
    let holder = Store::open(&path).unwrap();
    holder.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let draft = EventDraft::new(EventKind::SessionStatus, json!({"status": "PAUSED"}));
    let err = ctl.emit(vec![draft.clone()], "system").unwrap_err();
    assert!(err.to_lowercase().contains("busy") || err.to_lowercase().contains("locked"), "{err}");
    assert!(ctl.schedule().is_err(), "schedule must report a failed transaction too");
    assert_eq!(ctl.store.events("s1", 0, 10).unwrap().len(), 0, "no partial write may land");

    holder.conn.execute_batch("ROLLBACK").unwrap();
    drop(holder);
    ctl.emit(vec![draft], "system").unwrap();
    assert_eq!(ctl.store.events("s1", 0, 10).unwrap().len(), 1);

    drop(ctl);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

#[test]
fn reduce_failure_rolls_back_and_the_refusal_replays() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    // injected failure: the task row is written by reduce, the event write aborts
    ctl.store
        .conn
        .execute_batch(
            "CREATE TRIGGER boom BEFORE INSERT ON events WHEN NEW.kind='task_created'
             BEGIN SELECT RAISE(ABORT, 'boom'); END;",
        )
        .unwrap();
    let a = action("r1", "leader", ActionKind::AssignTask, json!({"assignee": "b", "description": "x"}), None);
    let receipt = ctl.submit(&a).unwrap();
    assert!(!receipt.ok);
    assert!(receipt.error.unwrap().contains("boom"));
    assert!(ctl.store.tasks_for_session("s1", &[]).unwrap().is_empty(), "the task row must roll back");

    // same action id: the recorded refusal replays, nothing runs twice
    let replay = ctl.submit(&a).unwrap();
    assert!(!replay.ok);
    assert!(ctl.store.tasks_for_session("s1", &[]).unwrap().is_empty());

    // new action id after the fault clears: succeeds on the clean state
    ctl.store.conn.execute_batch("DROP TRIGGER boom").unwrap();
    let r = ctl
        .submit(&action("r2", "leader", ActionKind::AssignTask, json!({"assignee": "b", "description": "x"}), None))
        .unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    assert_eq!(ctl.store.tasks_for_session("s1", &[]).unwrap().len(), 1);
}

#[test]
fn reduce_write_failure_rolls_back_the_shared_entry() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    // the write lands inside reduce, then the step fails
    ctl.store
        .conn
        .execute_batch(
            "CREATE TRIGGER boom_share BEFORE INSERT ON shared_entries
             BEGIN SELECT RAISE(ABORT, 'share boom'); END;",
        )
        .unwrap();
    let a = action("sh1", "b", ActionKind::PublishShared, json!({"space_id": "lib", "content": "partial"}), None);
    let receipt = ctl.submit(&a).unwrap();
    assert!(!receipt.ok);
    assert!(receipt.error.unwrap().contains("share boom"));
    assert!(ctl.store.shared_entries("s1", &["lib".to_string()], 0, 10).unwrap().is_empty());
    assert!(ctl.store.events("s1", 0, 100).unwrap().iter().all(|e| e["kind"] != json!("shared_published")));

    ctl.store.conn.execute_batch("DROP TRIGGER boom_share").unwrap();
    let r = ctl
        .submit(&action("sh2", "b", ActionKind::PublishShared, json!({"space_id": "lib", "content": "partial"}), None))
        .unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    assert_eq!(ctl.store.shared_entries("s1", &["lib".to_string()], 0, 10).unwrap().len(), 1);
}

#[test]
fn begin_run_emits_run_started_with_the_wake_reason() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    ctl.begin_run(&run.run_id).unwrap();

    let events = ctl.store.events("s1", 0, 100).unwrap();
    let started =
        events.iter().find(|e| e["kind"] == json!("run_started")).expect("run_started is emitted for every turn");
    assert_eq!(started["payload"]["run_id"], json!(run.run_id));
    assert_eq!(started["payload"]["agent_id"], json!("leader"));
    assert_eq!(started["payload"]["status"], json!("RUNNING"));
    assert_eq!(started["payload"]["wake"], json!("user_input"));
    assert_eq!(started["actor_id"], json!("leader"));
}

#[test]
fn begin_run_starts_the_task_before_announcing_the_run() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    ctl.submit(&action("e1", "leader", ActionKind::AssignTask, json!({"assignee": "b", "description": "x"}), None))
        .unwrap();
    let b_run = ctl
        .store
        .runs_for_session("s1", &[TurnStatus::Queued])
        .unwrap()
        .into_iter()
        .find(|r| r.agent_id == "b")
        .unwrap();
    let _ = leader_run;
    ctl.begin_run(&b_run.run_id).unwrap();

    let kinds: Vec<String> = ctl
        .store
        .events("s1", 0, 200)
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap_or_default().to_string())
        .collect();
    let started = kinds.iter().position(|k| k == "task_started").expect("task_started");
    let run_started = kinds.iter().position(|k| k == "run_started").expect("run_started");
    assert!(started < run_started, "task_started precedes run_started");
}

#[test]
fn wait_for_tasks_reports_results_keyed_by_task_id() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    let a = action(
        "w1",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "b", "description": "x"}),
        Some(leader_run.run_id.clone()),
    );
    let task_id = derived_task_id(&a);
    ctl.submit(&a).unwrap();
    // a terminal task makes wait_for_tasks answer instead of parking
    ctl.submit(&action("w2", "user", ActionKind::CancelTask, json!({"task_id": task_id}), None)).unwrap();
    let r = ctl
        .submit(&action(
            "w3",
            "leader",
            ActionKind::WaitForTasks,
            json!({"task_ids": [task_id]}),
            Some(leader_run.run_id.clone()),
        ))
        .unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    assert_eq!(r.result["waiting"], json!(false));
    let results = &r.result["results"];
    assert!(results.is_object(), "results are keyed by task id, got {results}");
    assert_eq!(results[&task_id]["task_id"], json!(task_id));
    assert_eq!(results[&task_id]["status"], json!("CANCELLED"));
}

#[test]
fn wake_info_reports_task_results_keyed_by_task_id() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    let a = action(
        "k1",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "b", "description": "x"}),
        Some(leader_run.run_id.clone()),
    );
    let task_id = derived_task_id(&a);
    ctl.submit(&a).unwrap();
    // a member message lands after the user_message, so the last input kind is not user_input
    ctl.submit(&action("k2", "b", ActionKind::SendMessage, json!({"target": "leader", "text": "hi"}), None)).unwrap();
    ctl.begin_run(&leader_run.run_id).unwrap();
    let r = ctl
        .submit(&action(
            "k3",
            "leader",
            ActionKind::WaitForTasks,
            json!({"task_ids": [task_id]}),
            Some(leader_run.run_id.clone()),
        ))
        .unwrap();
    assert!(r.ok && r.result["waiting"] == json!(true));

    let run = ctl.store.get_run(&leader_run.run_id).unwrap().unwrap();
    let wake = ctl.wake_info(&run);
    assert_eq!(wake["reason"], json!("task_results"));
    let results = &wake["payload"]["results"];
    assert!(results.is_object(), "snapshots are keyed by task id, got {results}");
    assert_eq!(results[&task_id]["status"], json!("PENDING"));
}

#[test]
fn publish_shared_needs_nonempty_content_or_ref() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let r = ctl
        .submit(&action("q1", "b", ActionKind::PublishShared, json!({"space_id": "lib", "content": ""}), None))
        .unwrap();
    assert!(!r.ok);
    assert!(r.error.unwrap().contains("needs content or a ref"));
    // a ref alone is enough (`content or ref`)
    let r = ctl
        .submit(&action(
            "q2",
            "b",
            ActionKind::PublishShared,
            json!({"space_id": "lib", "ref": "artifacts/x.md"}),
            None,
        ))
        .unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
}

#[test]
fn apply_patch_with_empty_operations_uses_the_stored_operations() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let r = ctl.submit(&action(
        "j1",
        "b",
        ActionKind::ProposeTeamChange,
        json!({"operations": [{"op": "add_channel", "channel": {"source": "leader", "targets": ["cx"], "mode": "message"}}]}),
        None,
    )).unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    let patch_id = r.result["patch_id"].as_str().unwrap().to_string();
    let rev = ctl.store.current_revision("s1").unwrap();

    // `p.get("operations") or patch.operations` — empty list falls back
    let r = ctl
        .submit(&action(
            "j2",
            "leader",
            ActionKind::ApplyTopologyPatch,
            json!({"patch_id": patch_id, "operations": []}),
            None,
        ))
        .unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    assert_eq!(ctl.store.current_revision("s1").unwrap(), rev + 1);
    assert!(ctl.store.load_team_spec("s1", None).unwrap().can_send("leader", "cx"));
}

#[test]
fn read_shared_rejects_malformed_paging_arguments() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let r = ctl
        .submit(&action(
            "x1",
            "leader",
            ActionKind::ReadShared,
            json!({"space_id": "lib", "after_sequence": "abc"}),
            None,
        ))
        .unwrap();
    assert!(!r.ok, "int('abc') is an error, not a silent 0");
    assert!(r.error.unwrap().contains("after_sequence"));
    let r = ctl
        .submit(&action("x2", "leader", ActionKind::ReadShared, json!({"space_id": "lib", "limit": null}), None))
        .unwrap();
    assert!(!r.ok);
    assert!(r.error.unwrap().contains("limit"));
    let r = ctl
        .submit(&action("x2b", "leader", ActionKind::ReadShared, json!({"space_id": "lib", "limit": -1}), None))
        .unwrap();
    assert!(!r.ok);
    assert!(r.error.unwrap().contains("positive"));
    // well-formed paging still works
    let r = ctl
        .submit(&action(
            "x3",
            "leader",
            ActionKind::ReadShared,
            json!({"space_id": "lib", "after_sequence": 0, "limit": 10}),
            None,
        ))
        .unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
}

#[test]
fn action_id_reuse_with_different_payload_is_rejected() {
    let mut ctl = harness();
    assert!(ctl.submit(&user("same", "first")).unwrap().ok);
    let reused = ctl.submit(&user("same", "second")).unwrap_err();
    assert!(reused.contains("different action data"));
    let mut spoof = user("same", "first");
    spoof.actor_id = "leader".into();
    assert!(ctl.submit(&spoof).unwrap_err().contains("different action data"));
    assert_eq!(ctl.store.events("s1", 0, 100).unwrap().len(), 1);
}

#[test]
fn payload_hash_matches_golden_wire_vectors() {
    // golden digests of the canonical payload JSON (sorted keys, ", " / ": " separators)
    assert_eq!(payload_hash(&plain_action("h1", json!({"text": "hi"}))), "1af583c098473ec00cf39fee4e4216af");
    assert_eq!(
        payload_hash(&plain_action("h2", json!({"b": [1, 2], "a": {"x": null}, "n": 1.5}))),
        "08947c8519320655305548f2afb726df"
    );
}

#[test]
fn delivery_batch_ledger_round_trips_through_finalize() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let pending = ctl.store.pending_deliveries("s1", "leader").unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["batch_no"], json!(1));
    let next: i64 = ctl
        .store
        .conn
        .query_row("SELECT next_batch_no FROM agent_runtime WHERE session_id='s1' AND agent_id='leader'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(next, 2, "the batch number is handed out and the ledger advanced");

    let run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    ctl.begin_run(&run.run_id).unwrap();
    let delivery_id = pending[0]["delivery_id"].as_i64().unwrap();
    ctl.finalize_run(&run.run_id, &completed(Some("hi")), &[delivery_id]).unwrap();
    assert_eq!(ctl.store.applied_batch("s1", "leader").unwrap(), 1, "ack advances last_applied_batch");
    assert_eq!(ctl.store.pending_deliveries("s1", "leader").unwrap().len(), 0);
}

#[test]
fn mid_turn_push_uses_scoped_observer_payload() {
    let spec: TeamSpec = serde_json::from_value(json!({
        "leader_id": "leader",
        "agents": [
            {"id": "leader", "name": "L", "role": "leader", "runtime_kind": "deepagents", "model_profile": "m"},
            {"id": "b", "name": "B", "role": "worker", "runtime_kind": "deepagents", "model_profile": "m"},
            {"id": "obs", "name": "O", "role": "observer", "runtime_kind": "deepagents", "model_profile": "m"}
        ],
        "channels": [{"source": "leader", "targets": ["b"], "mode": "task"}],
        "observers": [{"agent_id": "obs", "subjects": ["b"], "payload_scope": "status", "wake_policy": "on_event"}]
    }))
    .unwrap();
    let store = Store::open_memory().unwrap();
    store.create_session("s1", "/tmp", "approved_scope").unwrap();
    store.save_team_spec("s1", &spec).unwrap();
    let mut ctl = Control::new(store, "s1");
    for a in &spec.agents {
        ctl.store.ensure_agent("s1", &a.id).unwrap();
    }
    // obs is mid-turn (RUNNING run), so new deliveries become mid-turn pushes
    let ts = now();
    ctl.store
        .insert_run(&TurnRun {
            run_id: "run-obs".into(),
            session_id: "s1".into(),
            task_id: None,
            goal_id: None,
            agent_id: "obs".into(),
            config_revision: ctl.store.agent_config_revision("s1", "obs").unwrap(),
            topology_revision: ctl.store.current_revision("s1").unwrap(),
            status: TurnStatus::Running,
            input_delivery_ids: vec![],
            context_ref: None,
            external_turn_id: None,
            cancel_requested: false,
            waiting_on: vec![],
            created_at: ts,
            updated_at: ts,
        })
        .unwrap();

    let a = action(
        "a1",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "b", "description": "TOP-SECRET-DESCRIPTION"}),
        None,
    );
    let r = ctl.submit(&a).unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());

    let pushes = ctl.drain_mid_turn_pushes();
    assert_eq!(pushes.len(), 1);
    assert_eq!(pushes[0].0, "run-obs");
    assert!(!pushes[0].1.is_empty());
    for item in &pushes[0].1 {
        let rendered = item["payload"].to_string();
        assert!(!rendered.contains("TOP-SECRET-DESCRIPTION"), "mid-turn push leaked the unscoped payload: {rendered}");
        assert!(item["payload"].get("task_id").is_some(), "scoped payload keeps status keys: {rendered}");
    }
}
