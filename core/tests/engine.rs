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
    let ctl = Control::new(store, "s1");
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
    let r = ctl.submit(&action("v1", "leader", ActionKind::AssignTask, json!({"assignee": "ghost", "description": "x"}), None)).unwrap();
    assert!(!r.ok && r.error.unwrap().contains("unknown assignee"));
    // b cannot delegate (no task channel from b)
    let r = ctl.submit(&action("v2", "b", ActionKind::AssignTask, json!({"assignee": "leader", "description": "x"}), None)).unwrap();
    assert!(!r.ok && r.error.unwrap().contains("not allowed to assign"));
    // codex member only via leader — leader CAN, so check the reverse guard via update: use non-leader path
    // empty description refused
    let r = ctl.submit(&action("v3", "leader", ActionKind::AssignTask, json!({"assignee": "b", "description": "  "}), None)).unwrap();
    assert!(!r.ok && r.error.unwrap().contains("must not be empty"));
    // user cannot submit member actions
    let r = ctl.submit(&action("v4", "user", ActionKind::SendMessage, json!({"target": "b", "text": "x"}), None)).unwrap();
    assert!(!r.ok && r.error.unwrap().contains("local user cannot"));
    // unknown actor
    let r = ctl.submit(&action("v5", "ghost", ActionKind::SendMessage, json!({"target": "b", "text": "x"}), None)).unwrap();
    assert!(!r.ok && r.error.unwrap().contains("not a team member"));
    // b cannot message cx (no channel)
    let r = ctl.submit(&action("v6", "b", ActionKind::SendMessage, json!({"target": "cx", "text": "x"}), None)).unwrap();
    assert!(!r.ok);
    // publish to space without write access
    let r = ctl.submit(&action("v7", "leader", ActionKind::PublishShared, json!({"space_id": "lib", "content": "x"}), None)).unwrap();
    assert!(!r.ok && r.error.unwrap().contains("no write access"));
    // signal_done only by leader
    let r = ctl.submit(&action("v8", "b", ActionKind::SignalDone, json!({}), Some("run_x".into()))).unwrap();
    assert!(!r.ok);
}

#[test]
fn shared_publish_and_read_flow() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let r = ctl.submit(&action("s2", "b", ActionKind::PublishShared, json!({"space_id": "lib", "content": "findings"}), None)).unwrap();
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
        json!({"operations": [{"op": "add_channel", "channel": {"source": "b", "targets": ["cx"], "mode": "message"}}]}),
        None,
    )).unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    let patch_id = r.result["patch_id"].as_str().unwrap().to_string();

    // stale base rejected on apply path only after revision moves; apply now works (leader)
    let r = ctl.submit(&action("p2", "leader", ActionKind::ApplyTopologyPatch, json!({"patch_id": patch_id}), None)).unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    assert_eq!(ctl.store.current_revision("s1").unwrap(), rev + 1);
    let spec = ctl.store.load_team_spec("s1", None).unwrap();
    assert!(spec.can_send("b", "cx"));

    // rejecting an already-applied patch fails
    let r = ctl.submit(&action("p3", "leader", ActionKind::ApplyTopologyPatch, json!({"patch_id": patch_id}), None)).unwrap();
    assert!(!r.ok);
}

#[test]
fn cancel_task_without_run_cancels_immediately() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    let a = action("c1", "leader", ActionKind::AssignTask, json!({"assignee": "b", "description": "x"}), Some(leader_run.run_id));
    let task_id = derived_task_id(&a);
    ctl.submit(&a).unwrap();
    // cancel the (not yet started) task as the user
    let r = ctl.submit(&action("c2", "user", ActionKind::CancelTask, json!({"task_id": task_id}), None)).unwrap();
    assert!(r.ok);
    let task = ctl.store.get_task(&task_id).unwrap().unwrap();
    assert!(matches!(task.status, TaskStatus::Cancelled));
}

#[test]
fn signal_done_blocked_by_unfinished_work() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    // mark the leader run RUNNING so it can signal
    ctl.store.set_run_status(&leader_run.run_id, TurnStatus::Running).unwrap();
    let a = action("d1", "leader", ActionKind::AssignTask, json!({"assignee": "b", "description": "x"}), Some(leader_run.run_id.clone()));
    ctl.submit(&a).unwrap();
    let r = ctl.submit(&action("d2", "leader", ActionKind::SignalDone, json!({"summary": "done"}), Some(leader_run.run_id.clone()))).unwrap();
    assert!(!r.ok);
    assert!(r.error.unwrap().contains("not yet complete"));
    let blockers = r.result["blockers"].as_array().unwrap();
    assert!(blockers.iter().any(|b| b.as_str().unwrap().contains("unfinished tasks")));
}

// -- finalize_run (runtime.py::_finalize port) ----------------------------------

use teamagents_core::control::TurnOutcome;

fn completed(reply: Option<&str>) -> TurnOutcome {
    TurnOutcome { status: TurnStatus::Completed, error: None, note: None, reply_text: reply.map(str::to_string) }
}

#[test]
fn finalize_commits_task_and_wakes_waiter() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);

    // leader assigns to b; b's run is queued by schedule
    let a = action("f1", "leader", ActionKind::AssignTask, json!({"assignee": "b", "description": "report"}), Some(leader_run.run_id.clone()));
    let task_id = derived_task_id(&a);
    ctl.submit(&a).unwrap();

    // leader run must be RUNNING before wait_for_tasks can park it
    ctl.begin_run(&leader_run.run_id).unwrap();
    // leader waits on the task: its run parks in WAITING_TASK
    let r = ctl.submit(&action("f2", "leader", ActionKind::WaitForTasks, json!({"task_ids": [task_id]}), Some(leader_run.run_id.clone()))).unwrap();
    assert!(r.ok && r.result["waiting"] == json!(true));

    // b begins + completes with a completion request
    let b_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().into_iter().find(|r| r.agent_id == "b").unwrap();
    ctl.begin_run(&b_run.run_id).unwrap();
    let task = ctl.store.get_task(&task_id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running); // begin_run started the task

    ctl.submit(&action("f3", "b", ActionKind::CompleteTask, json!({"task_id": task_id, "result_refs": ["artifacts/report.md"], "summary": "wrote it"}), Some(b_run.run_id.clone()))).unwrap();
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
    let r = ctl.submit(&action("g1", "leader", ActionKind::SignalDone, json!({"summary": "answered"}), Some(run.run_id.clone()))).unwrap();
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
    let r = ctl.submit(&action("r2", "leader", ActionKind::AssignTask, json!({"assignee": "b", "description": "x"}), None)).unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    assert_eq!(ctl.store.tasks_for_session("s1", &[]).unwrap().len(), 1);
}

#[test]
fn reduce_write_failure_rolls_back_the_shared_entry() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    // test_p0_4_reduce_rollback.py: the write lands inside reduce, then the step fails
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
    let r = ctl.submit(&action("sh2", "b", ActionKind::PublishShared, json!({"space_id": "lib", "content": "partial"}), None)).unwrap();
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
    let started = events
        .iter()
        .find(|e| e["kind"] == json!("run_started"))
        .expect("run_started is emitted for every turn");
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
    ctl.submit(&action("e1", "leader", ActionKind::AssignTask, json!({"assignee": "b", "description": "x"}), None)).unwrap();
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
    assert!(started < run_started, "task_started precedes run_started (runtime.py order)");
}

#[test]
fn wait_for_tasks_reports_results_keyed_by_task_id() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    let a = action("w1", "leader", ActionKind::AssignTask, json!({"assignee": "b", "description": "x"}), Some(leader_run.run_id.clone()));
    let task_id = derived_task_id(&a);
    ctl.submit(&a).unwrap();
    // a terminal task makes wait_for_tasks answer instead of parking
    ctl.submit(&action("w2", "user", ActionKind::CancelTask, json!({"task_id": task_id}), None)).unwrap();
    let r = ctl.submit(&action("w3", "leader", ActionKind::WaitForTasks, json!({"task_ids": [task_id]}), Some(leader_run.run_id.clone()))).unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    assert_eq!(r.result["waiting"], json!(false));
    let results = &r.result["results"];
    assert!(results.is_object(), "control.py keys results by task id, got {results}");
    assert_eq!(results[&task_id]["task_id"], json!(task_id));
    assert_eq!(results[&task_id]["status"], json!("CANCELLED"));
}

#[test]
fn wake_info_reports_task_results_keyed_by_task_id() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    let a = action("k1", "leader", ActionKind::AssignTask, json!({"assignee": "b", "description": "x"}), Some(leader_run.run_id.clone()));
    let task_id = derived_task_id(&a);
    ctl.submit(&a).unwrap();
    // a member message lands after the user_message, so the last input kind is not user_input
    ctl.submit(&action("k2", "b", ActionKind::SendMessage, json!({"target": "leader", "text": "hi"}), None)).unwrap();
    ctl.begin_run(&leader_run.run_id).unwrap();
    let r = ctl.submit(&action("k3", "leader", ActionKind::WaitForTasks, json!({"task_ids": [task_id]}), Some(leader_run.run_id.clone()))).unwrap();
    assert!(r.ok && r.result["waiting"] == json!(true));

    let run = ctl.store.get_run(&leader_run.run_id).unwrap().unwrap();
    let wake = ctl.wake_info(&run);
    assert_eq!(wake["reason"], json!("task_results"));
    let results = &wake["payload"]["results"];
    assert!(results.is_object(), "runtime.py keys snapshots by task id, got {results}");
    assert_eq!(results[&task_id]["status"], json!("PENDING"));
}

#[test]
fn publish_shared_needs_nonempty_content_or_ref() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let r = ctl.submit(&action("q1", "b", ActionKind::PublishShared, json!({"space_id": "lib", "content": ""}), None)).unwrap();
    assert!(!r.ok);
    assert!(r.error.unwrap().contains("needs content or a ref"));
    // a ref alone is enough (control.py: `content or ref`)
    let r = ctl.submit(&action("q2", "b", ActionKind::PublishShared, json!({"space_id": "lib", "ref": "artifacts/x.md"}), None)).unwrap();
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
        json!({"operations": [{"op": "add_channel", "channel": {"source": "b", "targets": ["cx"], "mode": "message"}}]}),
        None,
    )).unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    let patch_id = r.result["patch_id"].as_str().unwrap().to_string();
    let rev = ctl.store.current_revision("s1").unwrap();

    // control.py: `p.get("operations") or patch.operations` — empty list falls back
    let r = ctl.submit(&action("j2", "leader", ActionKind::ApplyTopologyPatch, json!({"patch_id": patch_id, "operations": []}), None)).unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    assert_eq!(ctl.store.current_revision("s1").unwrap(), rev + 1);
    assert!(ctl.store.load_team_spec("s1", None).unwrap().can_send("b", "cx"));
}

#[test]
fn read_shared_rejects_malformed_paging_arguments() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go")).unwrap();
    let r = ctl.submit(&action("x1", "leader", ActionKind::ReadShared, json!({"space_id": "lib", "after_sequence": "abc"}), None)).unwrap();
    assert!(!r.ok, "int('abc') is ValueError in Python, not a silent 0");
    assert!(r.error.unwrap().contains("after_sequence"));
    let r = ctl.submit(&action("x2", "leader", ActionKind::ReadShared, json!({"space_id": "lib", "limit": null}), None)).unwrap();
    assert!(!r.ok);
    assert!(r.error.unwrap().contains("limit"));
    // well-formed paging still works
    let r = ctl.submit(&action("x3", "leader", ActionKind::ReadShared, json!({"space_id": "lib", "after_sequence": 0, "limit": 10}), None)).unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
}

#[test]
fn payload_hash_matches_python_json_dumps() {
    // both digests come from control.py::_payload_hash (json.dumps defaults: ", " / ": ")
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
        .query_row("SELECT next_batch_no FROM agent_runtime WHERE session_id='s1' AND agent_id='leader'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(next, 2, "the batch number is handed out and the ledger advanced");

    let run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    ctl.begin_run(&run.run_id).unwrap();
    let delivery_id = pending[0]["delivery_id"].as_i64().unwrap();
    ctl.finalize_run(&run.run_id, &completed(Some("hi")), &[delivery_id]).unwrap();
    assert_eq!(ctl.store.applied_batch("s1", "leader").unwrap(), 1, "ack advances last_applied_batch");
    assert_eq!(ctl.store.pending_deliveries("s1", "leader").unwrap().len(), 0);
}
