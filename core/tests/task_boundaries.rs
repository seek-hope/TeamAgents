//! Task requests and delayed completion must respect current ownership and state.

use serde_json::json;
use teamagents_core::control::{Control, TurnOutcome};
use teamagents_core::models::*;
use teamagents_core::storage::Store;

fn spec() -> TeamSpec {
    serde_json::from_value(json!({
        "leader_id": "leader",
        "agents": [
            {"id":"leader","name":"Leader","role":"leader","runtime_kind":"deepagents","model_profile":"m"},
            {"id":"b","name":"Worker","role":"worker","runtime_kind":"deepagents","model_profile":"m"}
        ],
        "channels": [
            {"source":"leader","targets":["b"],"mode":"task"},
            {"source":"b","targets":["leader"],"mode":"message"}
        ]
    }))
    .unwrap()
}

fn harness() -> Control {
    let store = Store::open_memory().unwrap();
    for session in ["s1", "s2"] {
        store.create_session(session, "/tmp", "approved_scope").unwrap();
        store.save_team_spec(session, &spec()).unwrap();
        for member in spec().agents {
            store.ensure_agent(session, &member.id).unwrap();
        }
    }
    Control::new(store, "s1")
}

fn action(id: &str, actor: &str, kind: ActionKind, payload: Json, run: Option<&str>) -> TeamAction {
    TeamAction {
        action_id: id.into(),
        session_id: "s1".into(),
        actor_id: actor.into(),
        kind,
        payload,
        run_id: run.map(str::to_string),
    }
}

fn assign(ctl: &mut Control, id: &str) -> String {
    let receipt = ctl
        .submit(&action(id, "leader", ActionKind::AssignTask, json!({"assignee":"b","description":id}), None))
        .unwrap();
    assert!(receipt.ok, "{receipt:?}");
    receipt.result["task_id"].as_str().unwrap().to_string()
}

fn start_worker(ctl: &mut Control) -> TurnRun {
    let run = ctl
        .store
        .runs_for_session("s1", &[TurnStatus::Queued])
        .unwrap()
        .into_iter()
        .find(|run| run.agent_id == "b")
        .unwrap();
    ctl.begin_run(&run.run_id).unwrap()
}

fn outcome(status: TurnStatus) -> TurnOutcome {
    TurnOutcome { status, error: None, note: None, reply_text: None }
}

fn complete(ctl: &mut Control, run: &TurnRun, task: &str) {
    let receipt = ctl
        .submit(&action(
            "complete",
            "b",
            ActionKind::CompleteTask,
            json!({"task_id":task,"result_refs":[],"summary":"ready"}),
            Some(&run.run_id),
        ))
        .unwrap();
    assert!(receipt.ok, "{receipt:?}");
}

fn completion_events(ctl: &Control, task: &str) -> Vec<Json> {
    ctl.store
        .events("s1", 0, 1000)
        .unwrap()
        .into_iter()
        .filter(|event| event["kind"] == "task_completed" && event["payload"]["task_id"] == task)
        .collect()
}

#[test]
fn finalization_reports_the_committed_status_and_preserves_settled_results() {
    for (reported, cancelled, expected) in [
        (TurnStatus::WaitingTask, true, TurnStatus::Cancelled),
        (TurnStatus::WaitingApproval, false, TurnStatus::Running),
        (TurnStatus::Completed, false, TurnStatus::Completed),
        (TurnStatus::Failed, false, TurnStatus::Failed),
    ] {
        let mut ctl = harness();
        let task = assign(&mut ctl, "work");
        let run = start_worker(&mut ctl);
        complete(&mut ctl, &run, &task);
        if cancelled {
            let receipt = ctl
                .submit(&action("cancel", "user", ActionKind::CancelRun, json!({"run_id":run.run_id}), None))
                .unwrap();
            assert!(receipt.ok);
        }
        let result = ctl.finalize_run(&run.run_id, &outcome(reported), &run.input_delivery_ids).unwrap();
        assert!(result.applied);
        assert_eq!(result.status, expected);
        assert_eq!(ctl.store.get_run(&run.run_id).unwrap().unwrap().status, result.status);
        if result.status.is_terminal() {
            let events = ctl.store.events("s1", 0, 1000).unwrap();
            let replay = ctl.finalize_run(&run.run_id, &outcome(TurnStatus::WaitingTask), &[]).unwrap();
            assert!(!replay.applied);
            assert_eq!(replay.status, result.status);
            assert_eq!(ctl.store.events("s1", 0, 1000).unwrap(), events);
        }
    }
}

#[test]
fn task_completion_cannot_resurrect_a_cancelled_sibling() {
    let mut ctl = harness();
    let own = assign(&mut ctl, "own");
    let run = start_worker(&mut ctl);
    let sibling = assign(&mut ctl, "sibling");
    complete(&mut ctl, &run, &sibling);
    let saved = ctl.store.completion_request(&run.run_id).unwrap();
    let receipt =
        ctl.submit(&action("cancel", "user", ActionKind::CancelTask, json!({"task_id":sibling}), None)).unwrap();
    assert!(receipt.ok, "{receipt:?}");
    assert_eq!(ctl.store.get_task(&sibling).unwrap().unwrap().status, TaskStatus::Cancelled);
    assert_eq!(ctl.store.get_run(&run.run_id).unwrap().unwrap().status, TurnStatus::Running);

    ctl.finalize_run(&run.run_id, &outcome(TurnStatus::Completed), &[]).unwrap();
    assert_eq!(ctl.store.get_task(&sibling).unwrap().unwrap().status, TaskStatus::Cancelled);
    assert_eq!(ctl.store.get_task(&own).unwrap().unwrap().status, TaskStatus::Blocked);
    assert!(completion_events(&ctl, &sibling).is_empty());
    assert_eq!(ctl.store.completion_request(&run.run_id).unwrap(), saved, "keep the rejected request for audit");
}

#[test]
fn task_completion_preserves_successful_sibling_delivery() {
    let mut ctl = harness();
    let own = assign(&mut ctl, "own");
    let run = start_worker(&mut ctl);
    let sibling = assign(&mut ctl, "sibling");
    complete(&mut ctl, &run, &sibling);
    ctl.finalize_run(&run.run_id, &outcome(TurnStatus::Completed), &[]).unwrap();
    assert_eq!(ctl.store.get_task(&sibling).unwrap().unwrap().status, TaskStatus::Succeeded);
    assert_eq!(ctl.store.get_task(&own).unwrap().unwrap().status, TaskStatus::Blocked);
    assert_eq!(completion_events(&ctl, &sibling).len(), 1);
}

#[test]
fn task_completion_rechecks_ownership_after_member_removal() {
    let mut ctl = harness();
    let task = assign(&mut ctl, "work");
    let run = start_worker(&mut ctl);
    complete(&mut ctl, &run, &task);
    ctl.finalize_run(&run.run_id, &outcome(TurnStatus::OutcomeUnknown), &[]).unwrap();
    let receipt = ctl
        .submit(&action(
            "remove",
            "leader",
            ActionKind::ApplyTopologyPatch,
            json!({"base_revision":1,"operations":[{"op":"remove_agent","agent_id":"b"}]}),
            None,
        ))
        .unwrap();
    assert!(receipt.ok, "{receipt:?}");
    assert_eq!(receipt.result["status"], "APPLIED");
    let handed = ctl.store.get_task(&task).unwrap().unwrap();
    assert_eq!(handed.assignee, "leader");
    assert_eq!(handed.status, TaskStatus::Blocked);
    assert_eq!(ctl.store.agent_status("s1", "b").unwrap(), Some(AgentStatus::Removed));

    ctl.finalize_run(&run.run_id, &outcome(TurnStatus::Completed), &[]).unwrap();
    assert_eq!(json!(ctl.store.get_task(&task).unwrap().unwrap()), json!(handed));
    assert!(completion_events(&ctl, &task).is_empty());
    assert_eq!(ctl.store.agent_status("s1", "b").unwrap(), Some(AgentStatus::Removed));
    assert_eq!(ctl.store.get_run(&run.run_id).unwrap().unwrap().status, TurnStatus::Completed);
}

#[test]
fn task_completion_can_reconcile_its_original_unknown_run() {
    let mut ctl = harness();
    let task = assign(&mut ctl, "work");
    let run = start_worker(&mut ctl);
    complete(&mut ctl, &run, &task);
    ctl.finalize_run(&run.run_id, &outcome(TurnStatus::OutcomeUnknown), &[]).unwrap();
    assert_eq!(ctl.store.get_task(&task).unwrap().unwrap().status, TaskStatus::Blocked);

    ctl.finalize_run(&run.run_id, &outcome(TurnStatus::Completed), &[]).unwrap();
    ctl.finalize_run(&run.run_id, &outcome(TurnStatus::Completed), &[]).unwrap();
    assert_eq!(ctl.store.get_task(&task).unwrap().unwrap().status, TaskStatus::Succeeded);
    assert_eq!(completion_events(&ctl, &task).len(), 1);
}

#[test]
fn task_completion_reconciliation_preserves_the_members_newer_run_status() {
    let mut ctl = harness();
    let task = assign(&mut ctl, "old");
    let old = start_worker(&mut ctl);
    complete(&mut ctl, &old, &task);
    ctl.finalize_run(&old.run_id, &outcome(TurnStatus::OutcomeUnknown), &[]).unwrap();
    let next = assign(&mut ctl, "new");
    let current = start_worker(&mut ctl);
    assert_eq!(current.task_id.as_deref(), Some(next.as_str()));
    assert_eq!(ctl.store.agent_status("s1", "b").unwrap(), Some(AgentStatus::Busy));
    ctl.finalize_run(&old.run_id, &outcome(TurnStatus::Completed), &[]).unwrap();
    assert_eq!(ctl.store.get_task(&task).unwrap().unwrap().status, TaskStatus::Succeeded);
    assert_eq!(ctl.store.get_task(&next).unwrap().unwrap().status, TaskStatus::Running);
    assert_eq!(ctl.store.get_run(&current.run_id).unwrap().unwrap().status, TurnStatus::Running);
    assert_eq!(ctl.store.agent_status("s1", "b").unwrap(), Some(AgentStatus::Busy));
}

fn foreign_work(ctl: &Control, status: TurnStatus) -> TurnRun {
    let task: Task = serde_json::from_value(json!({
        "task_id":"foreign-task","requester":"leader","assignee":"b","description":"foreign",
        "result_refs":["PRIVATE_FOREIGN_RESULT"],"status":"PENDING"
    }))
    .unwrap();
    ctl.store.insert_task("s2", &task).unwrap();
    let run = TurnRun {
        run_id: "foreign-run".into(),
        session_id: "s2".into(),
        task_id: Some(task.task_id),
        goal_id: None,
        agent_id: "b".into(),
        config_revision: 1,
        topology_revision: 1,
        status,
        input_delivery_ids: vec![],
        context_ref: None,
        external_turn_id: None,
        cancel_requested: false,
        waiting_on: vec![],
        created_at: now(),
        updated_at: now(),
    };
    ctl.store.insert_run(&run).unwrap();
    run
}

#[test]
fn queued_recovery_admission_cannot_cross_sessions_or_revive_a_settled_run() {
    let mut ctl = harness();
    assign(&mut ctl, "local");
    let local = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    let foreign = foreign_work(&ctl, TurnStatus::Queued);
    let member = ctl.store.agent_status("s1", &local.agent_id).unwrap();
    assert!(ctl.restore_queued_runs(&[local.run_id.clone(), foreign.run_id.clone()]).is_err());
    assert_eq!(json!(ctl.store.get_run(&local.run_id).unwrap().unwrap()), json!(local));
    assert_eq!(json!(ctl.store.get_run(&foreign.run_id).unwrap().unwrap()), json!(foreign));
    assert_eq!(ctl.store.agent_status("s1", &local.agent_id).unwrap(), member);
    ctl.finalize_run(&local.run_id, &outcome(TurnStatus::Cancelled), &local.input_delivery_ids).unwrap();
    let settled = ctl.store.get_run(&local.run_id).unwrap().unwrap();
    let events = ctl.store.events("s1", 0, 1000).unwrap();
    let restored = ctl.restore_queued_runs(&[local.run_id]).unwrap();
    assert_eq!(json!(restored), json!([settled]));
    assert_eq!(ctl.store.events("s1", 0, 1000).unwrap(), events);
}

#[test]
fn task_actions_reject_foreign_session_references() {
    for (kind, actor, payload, foreign_actor_run) in [
        (
            ActionKind::AssignTask,
            "leader",
            json!({"assignee":"b","description":"work","dependencies":["foreign-task"]}),
            false,
        ),
        (
            ActionKind::AssignTask,
            "leader",
            json!({"assignee":"b","description":"work","parent_task_id":"foreign-task"}),
            false,
        ),
        (ActionKind::CompleteTask, "b", json!({"task_id":"foreign-task"}), false),
        (ActionKind::WaitForTasks, "b", json!({"task_ids":["foreign-task"]}), false),
        (ActionKind::CancelTask, "user", json!({"task_id":"foreign-task"}), false),
        (ActionKind::CancelRun, "user", json!({"run_id":"foreign-run"}), false),
        (ActionKind::SendMessage, "b", json!({"target":"leader","text":"wrong run"}), true),
    ] {
        let mut ctl = harness();
        let task = assign(&mut ctl, "local");
        let run = start_worker(&mut ctl);
        foreign_work(&ctl, TurnStatus::Queued);
        let foreign_before = ctl.store.get_task("foreign-task").unwrap().unwrap();
        let events = ctl.store.events("s1", 0, 1000).unwrap();
        let input = action(
            "foreign-reference",
            actor,
            kind,
            payload,
            if foreign_actor_run {
                Some("foreign-run")
            } else if actor == "b" {
                Some(&run.run_id)
            } else {
                None
            },
        );
        let receipt = ctl.submit(&input).unwrap();
        assert!(!receipt.ok, "{kind:?} accepted another session's reference: {receipt:?}");
        assert_eq!(json!(ctl.submit(&input).unwrap()), json!(receipt));
        assert_eq!(json!(ctl.store.get_task("foreign-task").unwrap().unwrap()), json!(foreign_before));
        assert_eq!(ctl.store.get_task(&task).unwrap().unwrap().status, TaskStatus::Running);
        assert_eq!(ctl.store.events("s1", 0, 1000).unwrap(), events);
        assert!(ctl.store.events("s2", 0, 1000).unwrap().is_empty());
        assert!(!serde_json::to_string(&receipt).unwrap().contains("PRIVATE_FOREIGN_RESULT"));
    }
}

#[test]
fn task_run_lifecycle_rejects_foreign_session_without_side_effects() {
    for step in ["begin", "timeout", "finalize"] {
        let mut ctl = harness();
        let run = foreign_work(&ctl, if step == "timeout" { TurnStatus::Running } else { TurnStatus::Queued });
        let task = ctl.store.get_task("foreign-task").unwrap();
        let result = match step {
            "begin" => ctl.begin_run(&run.run_id).map(|_| ()),
            "timeout" => ctl.stop_timeout(&run.run_id),
            _ => ctl.finalize_run(&run.run_id, &outcome(TurnStatus::Failed), &[]).map(|_| ()),
        };
        assert!(result.is_err(), "{step} accepted another session's run");
        assert_eq!(json!(ctl.store.get_run(&run.run_id).unwrap().unwrap()), json!(run));
        assert_eq!(json!(ctl.store.get_task("foreign-task").unwrap()), json!(task));
        for session in ["s1", "s2"] {
            assert!(ctl.store.events(session, 0, 1000).unwrap().is_empty());
        }
    }
}

#[test]
fn task_run_cannot_start_with_a_settled_reassigned_or_foreign_task() {
    for mutation in ["settled", "reassigned", "foreign"] {
        let mut ctl = harness();
        let task = assign(&mut ctl, "work");
        let run = ctl
            .store
            .runs_for_session("s1", &[TurnStatus::Queued])
            .unwrap()
            .into_iter()
            .find(|run| run.agent_id == "b")
            .unwrap();
        match mutation {
            "settled" => {
                ctl.store.compare_and_set_task(&task, "PENDING", TaskStatus::Cancelled, None).unwrap();
            }
            "reassigned" => {
                ctl.store.reassign_tasks("s1", "b", "leader", &[TaskStatus::Pending]).unwrap();
            }
            _ => {
                foreign_work(&ctl, TurnStatus::Queued);
                ctl.store
                    .conn
                    .execute("UPDATE turn_runs SET task_id='foreign-task' WHERE run_id=?1", [&run.run_id])
                    .unwrap();
            }
        }
        let before = json!({
            "run":ctl.store.get_run(&run.run_id).unwrap(),
            "tasks":ctl.store.tasks_for_session("s1", &[]).unwrap(),
            "foreign":ctl.store.tasks_for_session("s2", &[]).unwrap(),
            "events":ctl.store.events("s1", 0, 1000).unwrap(),
            "member":ctl.store.agent_status("s1", "b").unwrap(),
        });
        assert!(ctl.begin_run(&run.run_id).is_err(), "{mutation} task was started");
        assert_eq!(
            json!({
                "run":ctl.store.get_run(&run.run_id).unwrap(),
                "tasks":ctl.store.tasks_for_session("s1", &[]).unwrap(),
                "foreign":ctl.store.tasks_for_session("s2", &[]).unwrap(),
                "events":ctl.store.events("s1", 0, 1000).unwrap(),
                "member":ctl.store.agent_status("s1", "b").unwrap(),
            }),
            before
        );
    }
}

#[test]
fn task_completion_keeps_terminal_results_and_rejects_new_requests_from_settled_runs() {
    for terminal in [TaskStatus::Succeeded, TaskStatus::Failed, TaskStatus::Cancelled] {
        let mut ctl = harness();
        let task = assign(&mut ctl, "work");
        let run = start_worker(&mut ctl);
        complete(&mut ctl, &run, &task);
        ctl.store.compare_and_set_task(&task, "RUNNING", terminal, Some(&["original.md".into()])).unwrap();
        let original = json!(ctl.store.get_task(&task).unwrap());
        ctl.finalize_run(&run.run_id, &outcome(TurnStatus::Completed), &run.input_delivery_ids).unwrap();
        assert_eq!(json!(ctl.store.get_task(&task).unwrap()), original);
        assert!(completion_events(&ctl, &task).is_empty());

        let new_task = assign(&mut ctl, "new-work");
        let request =
            action("stale-run", "b", ActionKind::CompleteTask, json!({"task_id":new_task}), Some(&run.run_id));
        let saved = ctl.store.completion_request(&run.run_id).unwrap();
        let refused = ctl.submit(&request).unwrap();
        assert!(!refused.ok, "a settled run accepted another completion: {refused:?}");
        assert_eq!(ctl.store.completion_request(&run.run_id).unwrap(), saved);
    }
}

#[test]
fn task_handover_and_stale_waits_stay_in_their_session() {
    let mut ctl = harness();
    let foreign = foreign_work(&ctl, TurnStatus::Queued);
    let before = json!(ctl.store.get_task("foreign-task").unwrap());
    assert!(
        ctl.submit(&action(
            "remove",
            "leader",
            ActionKind::ApplyTopologyPatch,
            json!({"base_revision":1,"operations":[{"op":"remove_agent","agent_id":"b"}]}),
            None,
        ))
        .unwrap()
        .ok
    );
    assert_eq!(json!(ctl.store.get_task("foreign-task").unwrap()), before);
    assert_eq!(json!(ctl.store.get_run("foreign-run").unwrap()), json!(Some(&foreign)));

    let mut wait = foreign.clone();
    wait.session_id = "s1".into();
    wait.agent_id = "leader".into();
    wait.run_id = "local-stale-wait".into();
    wait.waiting_on = vec!["foreign-task".into()];
    assert_eq!(
        ctl.wake_info(&wait).unwrap()["payload"]["results"]["foreign-task"],
        json!({"task_id":"foreign-task","status":"UNKNOWN"})
    );
    assert!(ctl.confirmed_delivery_ids(&foreign.run_id).is_err());
}

#[test]
fn task_finalization_read_and_audit_failures_roll_back_and_can_be_retried() {
    for corrupt in [true, false] {
        let mut ctl = harness();
        let own = assign(&mut ctl, "own");
        let run = start_worker(&mut ctl);
        let sibling = assign(&mut ctl, "sibling");
        complete(&mut ctl, &run, &sibling);
        if corrupt {
            ctl.store.conn.execute("UPDATE tasks SET status='BROKEN' WHERE task_id=?1", [&sibling]).unwrap();
        } else {
            assert!(
                ctl.submit(&action("cancel", "user", ActionKind::CancelTask, json!({"task_id":sibling}), None))
                    .unwrap()
                    .ok
            );
            ctl.store
                .conn
                .execute_batch(
                    "CREATE TRIGGER reject_completion_audit BEFORE INSERT ON events
                     WHEN NEW.kind='run_progress' BEGIN SELECT RAISE(ABORT,'audit unavailable'); END;",
                )
                .unwrap();
        }
        let events = ctl.store.events("s1", 0, 1000).unwrap();
        let deliveries = ctl.store.pending_deliveries("s1", "b").unwrap();
        let saved = ctl.store.completion_request(&run.run_id).unwrap();
        assert!(ctl.finalize_run(&run.run_id, &outcome(TurnStatus::Completed), &run.input_delivery_ids).is_err());
        assert_eq!(ctl.store.get_run(&run.run_id).unwrap().unwrap().status, TurnStatus::Running);
        assert_eq!(ctl.store.get_task(&own).unwrap().unwrap().status, TaskStatus::Running);
        assert_eq!(ctl.store.events("s1", 0, 1000).unwrap(), events);
        assert_eq!(ctl.store.pending_deliveries("s1", "b").unwrap(), deliveries);
        assert_eq!(ctl.store.completion_request(&run.run_id).unwrap(), saved);
        if corrupt {
            ctl.store.conn.execute("UPDATE tasks SET status='PENDING' WHERE task_id=?1", [&sibling]).unwrap();
        } else {
            ctl.store.conn.execute_batch("DROP TRIGGER reject_completion_audit").unwrap();
        }
        ctl.finalize_run(&run.run_id, &outcome(TurnStatus::Completed), &run.input_delivery_ids).unwrap();
        assert_eq!(ctl.store.get_task(&own).unwrap().unwrap().status, TaskStatus::Blocked);
        assert_eq!(
            ctl.store.get_task(&sibling).unwrap().unwrap().status,
            if corrupt { TaskStatus::Succeeded } else { TaskStatus::Cancelled }
        );
    }
}

#[test]
fn task_requests_refuse_malformed_fields_without_changing_work() {
    let mut ctl = harness();
    let task = assign(&mut ctl, "work");
    let run = start_worker(&mut ctl);
    complete(&mut ctl, &run, &task);
    let before = json!({
        "tasks":ctl.store.tasks_for_session("s1", &[]).unwrap(),
        "runs":ctl.store.runs_for_session("s1", &[]).unwrap(),
        "events":ctl.store.events("s1", 0, 1000).unwrap(),
        "completion":ctl.store.completion_request(&run.run_id).unwrap(),
    });
    for (index, (kind, actor, payload)) in [
        (ActionKind::AssignTask, "leader", json!({"assignee":"b","description":true})),
        (ActionKind::AssignTask, "leader", json!({"assignee":"b","description":"work","dependencies":"wrong"})),
        (ActionKind::AssignTask, "leader", json!({"assignee":"b","description":"work","dependencies":[false]})),
        (ActionKind::AssignTask, "leader", json!({"assignee":"b","description":"work","dependencies":null})),
        (ActionKind::AssignTask, "leader", json!({"assignee":"b","description":"work","acceptance":{}})),
        (ActionKind::AssignTask, "leader", json!({"assignee":"b","description":"work","task_id":""})),
        (ActionKind::AssignTask, "leader", json!({"assignee":"b","description":"work","task_id":false})),
        (ActionKind::AssignTask, "leader", json!({"assignee":"b","description":"work","parent_task_id":false})),
        (ActionKind::AssignTask, "leader", json!({"assignee":"b","description":"work","extra":true})),
        (ActionKind::CompleteTask, "b", json!({"task_id":task,"summary":false})),
        (ActionKind::CompleteTask, "b", json!({"task_id":task,"result_refs":null})),
        (ActionKind::CompleteTask, "b", json!({"task_id":task,"extra":true})),
        (ActionKind::WaitForTasks, "b", json!({})),
        (ActionKind::WaitForTasks, "b", json!({"task_ids":"wrong"})),
        (ActionKind::WaitForTasks, "b", json!({"task_ids":[task, false]})),
        (ActionKind::WaitForTasks, "b", json!({"task_ids":[],"extra":true})),
        (ActionKind::CancelTask, "user", json!({"task_id":task,"extra":true})),
        (ActionKind::CancelRun, "user", json!({"run_id":run.run_id,"extra":true})),
    ]
    .into_iter()
    .enumerate()
    {
        let input = action(
            &format!("malformed-{index}"),
            actor,
            kind,
            payload,
            if actor == "b" { Some(&run.run_id) } else { None },
        );
        let receipt = ctl.submit(&input).unwrap();
        assert!(!receipt.ok, "malformed request {input:?} was accepted: {receipt:?}");
        assert_eq!(json!(ctl.submit(&input).unwrap()), json!(receipt));
        assert_eq!(
            json!({
                "tasks":ctl.store.tasks_for_session("s1", &[]).unwrap(),
                "runs":ctl.store.runs_for_session("s1", &[]).unwrap(),
                "events":ctl.store.events("s1", 0, 1000).unwrap(),
                "completion":ctl.store.completion_request(&run.run_id).unwrap(),
            }),
            before
        );
    }
    let corrected = ctl
        .submit(&action(
            "corrected",
            "leader",
            ActionKind::AssignTask,
            json!({"assignee":"b","description":"next","task_id":"explicit-task","parent_task_id":task,
                "dependencies":[task],"acceptance":"verified"}),
            None,
        ))
        .unwrap();
    assert!(corrected.ok, "{corrected:?}");
    assert_eq!(corrected.result["task_id"], "explicit-task");
    assert_eq!(ctl.store.get_task("explicit-task").unwrap().unwrap().parent_task_id.as_deref(), Some(task.as_str()));
}

#[test]
fn task_finalization_refuses_acknowledging_another_sessions_delivery() {
    let mut ctl = harness();
    let task = assign(&mut ctl, "work");
    let run = start_worker(&mut ctl);
    complete(&mut ctl, &run, &task);
    ctl.session_id = "s2".into();
    let mut foreign_action =
        action("foreign-assign", "leader", ActionKind::AssignTask, json!({"assignee":"b","description":"other"}), None);
    foreign_action.session_id = "s2".into();
    assert!(ctl.submit(&foreign_action).unwrap().ok);
    let foreign = ctl.store.pending_deliveries("s2", "b").unwrap();
    let foreign_id = foreign[0]["delivery_id"].as_i64().unwrap();
    ctl.session_id = "s1".into();
    let events = ctl.store.events("s1", 0, 1000).unwrap();
    let deliveries = ctl.store.pending_deliveries("s1", "b").unwrap();
    assert!(ctl.finalize_run(&run.run_id, &outcome(TurnStatus::Completed), &[foreign_id]).is_err());
    assert_eq!(ctl.store.get_task(&task).unwrap().unwrap().status, TaskStatus::Running);
    assert_eq!(ctl.store.get_run(&run.run_id).unwrap().unwrap().status, TurnStatus::Running);
    assert_eq!(ctl.store.events("s1", 0, 1000).unwrap(), events);
    assert_eq!(ctl.store.pending_deliveries("s1", "b").unwrap(), deliveries);
    assert_eq!(ctl.store.pending_deliveries("s2", "b").unwrap(), foreign);
    ctl.finalize_run(&run.run_id, &outcome(TurnStatus::Completed), &run.input_delivery_ids).unwrap();
    assert_eq!(ctl.store.get_task(&task).unwrap().unwrap().status, TaskStatus::Succeeded);
    assert_eq!(ctl.store.pending_deliveries("s2", "b").unwrap(), foreign);
}

#[test]
fn task_server_endpoints_cannot_read_or_update_a_foreign_run() {
    use teamagents_core::server::Server;
    let ctl = harness();
    let foreign = foreign_work(&ctl, TurnStatus::Queued);
    ctl.store.record_completion_request(&foreign.run_id, "foreign-task", &[], "PRIVATE_COMPLETION").unwrap();
    ctl.store.set_meta("confirmed_input:foreign-run", "[123]").unwrap();
    let approval = ApprovalRequest {
        approval_id: "foreign-approval".into(),
        session_id: "s2".into(),
        agent_id: "b".into(),
        run_id: foreign.run_id.clone(),
        tool_call_id: "call".into(),
        operation_hash: "hash".into(),
        requested_scope: json!({"private":"PRIVATE_APPROVAL"}),
        policy_revision: 1,
        status: ApprovalStatus::ApprovedOnce,
        created_at: now(),
        decided_at: None,
    };
    ctl.store.insert_approval(&approval).unwrap();
    let mut server = Server::new(":memory:");
    server.controls.insert("s1".into(), ctl);
    for method in [
        "begin_run",
        "finalize_run",
        "stop_timeout",
        "requeue_run",
        "set_run_status",
        "set_run_external_turn",
        "get_completion_request",
        "confirmed_delivery_ids",
        "confirm_delivery_ids",
    ] {
        let result = server.dispatch(
            method,
            &json!({"session_id":"s1","run_id":foreign.run_id,"status":"FAILED",
                "external_turn_id":"replacement","delivery_ids":[]}),
        );
        assert!(result.is_err(), "{method} accepted a foreign run: {result:?}");
        assert!(!format!("{result:?}").contains("PRIVATE_"));
        assert_eq!(json!(server.controls["s1"].store.get_run(&foreign.run_id).unwrap()), json!(Some(&foreign)));
    }
    for method in ["approval_for_call", "approval_find_run"] {
        let result = server
            .dispatch(
                method,
                &json!({"session_id":"s1","run_id":foreign.run_id,"tool_call_id":"call","operation_hash":"hash"}),
            )
            .unwrap();
        assert!(result["approval"].is_null(), "{method} leaked a foreign approval: {result}");
    }
    let mut forged = approval.clone();
    forged.approval_id = "forged".into();
    forged.session_id = "s1".into();
    for request in [approval, forged] {
        assert!(server.dispatch("insert_approval", &json!({"session_id":"s1","approval":request})).is_err());
    }
    let ctl = &server.controls["s1"];
    assert!(ctl.store.events("s1", 0, 1000).unwrap().is_empty());
    assert!(ctl.store.get_approval_for_session("s1", "forged").unwrap().is_none());
}

#[test]
fn task_refusals_and_stale_completion_survive_database_reopen() {
    struct Directory(std::path::PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let root = Directory(std::env::temp_dir().join(format!("ta-task-reopen-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(&root.0).unwrap();
    let database = root.0.join("team.db");
    let mut ctl = harness();
    assign(&mut ctl, "own");
    let run = start_worker(&mut ctl);
    let sibling = assign(&mut ctl, "sibling");
    complete(&mut ctl, &run, &sibling);
    let invalid =
        action("bad", "b", ActionKind::CompleteTask, json!({"task_id":sibling,"summary":false}), Some(&run.run_id));
    let refusal = ctl.submit(&invalid).unwrap();
    assert!(!refusal.ok);
    assert!(
        ctl.submit(&action("cancel", "user", ActionKind::CancelTask, json!({"task_id":sibling}), None)).unwrap().ok
    );
    ctl.store.conn.execute("VACUUM INTO ?1", [database.to_str().unwrap()]).unwrap();
    drop(ctl);
    let mut restored = Control::new(Store::open(&database).unwrap(), "s1");
    assert_eq!(json!(restored.submit(&invalid).unwrap()), json!(refusal));
    let mut changed = invalid.clone();
    changed.payload["summary"] = json!("changed");
    assert!(restored.submit(&changed).is_err(), "changing an already used action must not replace it");
    restored.finalize_run(&run.run_id, &outcome(TurnStatus::Completed), &run.input_delivery_ids).unwrap();
    assert_eq!(restored.store.get_task(&sibling).unwrap().unwrap().status, TaskStatus::Cancelled);
    assert!(completion_events(&restored, &sibling).is_empty());
    assert!(restored.store.completion_request(&run.run_id).unwrap().is_some());
    let events = restored.store.events("s1", 0, 1000).unwrap();
    drop(restored);
    let mut restored = Control::new(Store::open(&database).unwrap(), "s1");
    restored.finalize_run(&run.run_id, &outcome(TurnStatus::Completed), &run.input_delivery_ids).unwrap();
    assert_eq!(restored.store.events("s1", 0, 1000).unwrap(), events);
}
