//! Corrupt persisted work must fail without losing inputs or committing partial transitions.

use rusqlite::Connection;
use serde_json::json;
use teamagents_core::control::{Control, TurnOutcome};
use teamagents_core::models::*;
use teamagents_core::storage::Store;

fn harness() -> Control {
    let store = Store::open_memory().unwrap();
    let spec: TeamSpec = serde_json::from_value(json!({
        "leader_id":"leader",
        "agents":[
            {"id":"leader","name":"Leader","role":"leader","runtime_kind":"deepagents","model_profile":"m"},
            {"id":"b","name":"Worker","role":"worker","runtime_kind":"deepagents","model_profile":"m"}
        ],
        "channels":[{"source":"leader","targets":["b"],"mode":"task"}],
        "shared_spaces":[{"id":"main","readers":["leader"],"writers":["leader"]}]
    }))
    .unwrap();
    store.create_session("s", "/tmp", "approved_scope").unwrap();
    store.save_team_spec("s", &spec).unwrap();
    for member in &spec.agents {
        store.ensure_agent("s", &member.id).unwrap();
    }
    Control::new(store, "s")
}

fn action(id: &str, actor: &str, kind: ActionKind, payload: Json) -> TeamAction {
    TeamAction { action_id: id.into(), session_id: "s".into(), actor_id: actor.into(), run_id: None, kind, payload }
}

/// Read raw SQLite values so the snapshot itself does not hide corrupt JSON.
fn snapshot(conn: &Connection) -> Json {
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut state = serde_json::Map::new();
    for table in tables {
        let mut statement = conn.prepare(&format!("SELECT * FROM \"{table}\" ORDER BY rowid")).unwrap();
        let columns = statement.column_count();
        let rows: Vec<Vec<String>> = statement
            .query_map([], |row| {
                (0..columns).map(|index| row.get_ref(index).map(|value| format!("{value:?}"))).collect()
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        state.insert(table, json!(rows));
    }
    json!(state)
}

fn seed_task(ctl: &Control, id: &str, status: TaskStatus, dependencies: &[&str]) {
    let task: Task = serde_json::from_value(json!({
        "task_id":id,"requester":"leader","assignee":"b","description":id,
        "status":status,"dependencies":dependencies,"created_at":1234.5,"updated_at":1234.5
    }))
    .unwrap();
    ctl.store.insert_task("s", &task).unwrap();
}

fn queued_leader(ctl: &mut Control) -> TurnRun {
    assert!(
        ctl.submit(&action("input", "user", ActionKind::UserMessage, json!({"text":"preserve input"}))).unwrap().ok
    );
    ctl.store.runs_for_session("s", &[TurnStatus::Queued]).unwrap().remove(0)
}

fn approval(ctl: &Control, run: &TurnRun, id: &str, status: ApprovalStatus, created: f64) {
    ctl.store
        .insert_approval(&ApprovalRequest {
            approval_id: id.into(),
            session_id: "s".into(),
            agent_id: run.agent_id.clone(),
            run_id: run.run_id.clone(),
            tool_call_id: id.into(),
            operation_hash: id.into(),
            requested_scope: json!({"tool":"shell"}),
            policy_revision: 1,
            status,
            created_at: created,
            decided_at: None,
        })
        .unwrap();
}

fn outcome() -> TurnOutcome {
    TurnOutcome { status: TurnStatus::Completed, error: None, note: None, reply_text: None }
}

fn old_events(store: &Store, session_id: &str, count: usize) {
    for index in 0..count {
        let event = serde_json::from_value(json!({
            "event_id":format!("{session_id}-old-{index}"),
            "session_id":session_id,"actor_id":"leader","kind":"message",
            "payload":{"text":"retained history"},"created_at":now() - 40.0 * 86_400.0
        }))
        .unwrap();
        store.append_event(&event).unwrap();
    }
}

#[test]
fn run_start_evidence_is_not_limited_to_display_history_or_other_sessions() {
    let mut ctl = harness();
    let queued = queued_leader(&mut ctl);
    old_events(&ctl.store, "s", 1001);
    ctl.begin_run(&queued.run_id).unwrap();
    assert!(ctl.store.events("s", 0, 1000).unwrap().iter().all(|event| event["kind"] != "run_started"));
    ctl.store.create_session("other", "/tmp", "approved_scope").unwrap();
    let mut other_run = queued.clone();
    other_run.run_id = "other-run".into();
    other_run.session_id = "other".into();
    ctl.store.insert_run(&other_run).unwrap();
    ctl.store
        .append_event(
            &serde_json::from_value(json!({
                "event_id":"other-start","session_id":"other","actor_id":"leader","kind":"run_started",
                "payload":{"run_id":"other-run","agent_id":"leader","status":"RUNNING"}
            }))
            .unwrap(),
        )
        .unwrap();
    let wanted = vec![queued.run_id.clone(), other_run.run_id.clone()];
    assert_eq!(ctl.store.run_started_ids("s", &wanted).unwrap(), [queued.run_id.clone()].into());
    assert_eq!(ctl.store.run_started_ids("other", &wanted).unwrap(), [other_run.run_id].into());
    ctl.store.conn.execute("UPDATE events SET payload_json='[' WHERE event_id='other-start'", []).unwrap();
    let before = snapshot(&ctl.store.conn);
    assert_eq!(ctl.store.run_started_ids("s", &wanted).unwrap(), [queued.run_id].into());
    assert!(ctl.store.run_started_ids("other", &wanted).is_err());
    assert_eq!(snapshot(&ctl.store.conn), before);
}

#[test]
fn history_retention_keeps_start_evidence_until_a_run_is_settled() {
    for status in [
        TurnStatus::Queued,
        TurnStatus::Running,
        TurnStatus::WaitingTask,
        TurnStatus::WaitingApproval,
        TurnStatus::OutcomeUnknown,
        TurnStatus::Completed,
        TurnStatus::Failed,
        TurnStatus::Cancelled,
    ] {
        let mut ctl = harness();
        let queued = queued_leader(&mut ctl);
        ctl.begin_run(&queued.run_id).unwrap();
        ctl.store.set_run_status(&queued.run_id, status).unwrap();
        ctl.store
            .conn
            .execute("UPDATE events SET created_at=?1 WHERE kind='run_started'", [now() - 40.0 * 86_400.0])
            .unwrap();
        let settled = matches!(status, TurnStatus::Completed | TurnStatus::Failed | TurnStatus::Cancelled);
        let before = snapshot(&ctl.store.conn);
        let (_, removable, _) = ctl.store.prune_history("s", 30, true).unwrap();
        assert_eq!(removable, i64::from(settled), "{status:?}");
        assert_eq!(snapshot(&ctl.store.conn), before);
        let (_, removed, _) = ctl.store.prune_history("s", 30, false).unwrap();
        assert_eq!(removed, removable);
        let retained: i64 = ctl
            .store
            .conn
            .query_row("SELECT COUNT(*) FROM events WHERE kind='run_started'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(retained, i64::from(!settled), "{status:?}");
    }
}

#[test]
fn history_retention_failure_does_not_partially_delete_delivery_evidence() {
    let mut ctl = harness();
    let queued = queued_leader(&mut ctl);
    // Put the failing delete beyond the first two parameter batches.
    old_events(&ctl.store, "s", 600);
    ctl.store.create_session("other", "/tmp", "approved_scope").unwrap();
    old_events(&ctl.store, "other", 3);
    let run = ctl.begin_run(&queued.run_id).unwrap();
    ctl.finalize_run(&run.run_id, &outcome(), &run.input_delivery_ids).unwrap();
    ctl.store.conn.execute("UPDATE events SET created_at=?1", [now() - 40.0 * 86_400.0]).unwrap();
    ctl.store.conn.execute("UPDATE deliveries SET created_at=?1", [now() - 40.0 * 86_400.0]).unwrap();
    ctl.store
        .conn
        .execute_batch(
            "CREATE TRIGGER refuse_history_delete BEFORE DELETE ON events WHEN OLD.kind='run_started'
         BEGIN SELECT RAISE(ABORT, 'history delete unavailable'); END;",
        )
        .unwrap();
    let before = snapshot(&ctl.store.conn);
    assert!(ctl.store.prune_history("s", 30, false).unwrap_err().to_string().contains("history delete unavailable"));
    assert_eq!(snapshot(&ctl.store.conn), before);
    ctl.store.conn.execute_batch("DROP TRIGGER refuse_history_delete").unwrap();
    let expected = ctl.store.prune_history("s", 30, true).unwrap();
    let (deliveries, events, _) = ctl.store.prune_history("s", 30, false).unwrap();
    assert_eq!((deliveries, events), (expected.0, expected.1));
    assert!(deliveries > 0 && events > 600);
    assert!(ctl.store.events("s", 0, 1000).unwrap().is_empty());
    assert_eq!(ctl.store.events("other", 0, 1000).unwrap().len(), 3);
}

#[test]
fn history_retention_rejects_malformed_start_evidence_without_deleting_it() {
    for case in 0..13 {
        let mut ctl = harness();
        let queued = queued_leader(&mut ctl);
        ctl.begin_run(&queued.run_id).unwrap();
        ctl.store.create_session("other", "/tmp", "approved_scope").unwrap();
        let mut foreign = queued.clone();
        foreign.run_id = "outside-session".into();
        foreign.session_id = "other".into();
        ctl.store.insert_run(&foreign).unwrap();
        let payload: String = ctl
            .store
            .conn
            .query_row("SELECT payload_json FROM events WHERE kind='run_started'", [], |row| row.get(0))
            .unwrap();
        let mut corrupt: Json = serde_json::from_str(&payload).unwrap();
        let mut actor = "leader";
        let bad = match case {
            0 => "[".into(),
            1 => "null".into(),
            2 => "[]".into(),
            3 => "{}".into(),
            _ => {
                match case {
                    4 => corrupt["run_id"] = json!(false),
                    5 => corrupt["run_id"] = json!(""),
                    6 => corrupt["agent_id"] = json!(""),
                    7 => corrupt["agent_id"] = json!("b"),
                    8 => corrupt["status"] = json!("COMPLETED"),
                    9 => corrupt["status"] = json!("PRIVATE_PAYLOAD_SENTINEL"),
                    10 => actor = "b",
                    11 => corrupt["run_id"] = json!("outside-session"),
                    12 => corrupt["run_id"] = json!("absent-run"),
                    _ => unreachable!(),
                }
                corrupt.to_string()
            }
        };
        ctl.store
            .conn
            .execute(
                "UPDATE events SET payload_json=?1,created_at=?2,actor_id=?3 WHERE kind='run_started'",
                rusqlite::params![bad, now() - 40.0 * 86_400.0, actor],
            )
            .unwrap();
        let before = snapshot(&ctl.store.conn);
        let wanted = vec![queued.run_id.clone(), "outside-session".into(), "absent-run".into()];
        let error = ctl.store.run_started_ids("s", &wanted).unwrap_err().to_string();
        assert!(error.contains("run_started") && !error.contains("PRIVATE_PAYLOAD_SENTINEL"), "{error}");
        for dry in [true, false] {
            let error = ctl.store.prune_history("s", 30, dry).unwrap_err().to_string();
            assert!(error.contains("run_started") && !error.contains("PRIVATE_PAYLOAD_SENTINEL"), "{error}");
            assert_eq!(snapshot(&ctl.store.conn), before);
        }
        ctl.store
            .conn
            .execute("UPDATE events SET payload_json=?1,actor_id='leader' WHERE kind='run_started'", [&payload])
            .unwrap();
        assert_eq!(ctl.store.run_started_ids("s", &wanted).unwrap(), [queued.run_id].into());
        assert_eq!(ctl.store.prune_history("s", 30, false).unwrap(), (0, 0, false));
    }
}

#[test]
fn queued_recovery_admission_rolls_back_the_entire_batch_on_storage_failure() {
    let mut ctl = harness();
    let first = queued_leader(&mut ctl);
    let mut second = first.clone();
    second.run_id = "queued-peer".into();
    second.agent_id = "b".into();
    second.input_delivery_ids.clear();
    ctl.store.insert_run(&second).unwrap();
    ctl.store.set_agent_status("s", "b", AgentStatus::Draining).unwrap();
    ctl.store.conn.execute("UPDATE turn_runs SET cancel_requested=1", []).unwrap();
    ctl.store
        .conn
        .execute_batch(
            "CREATE TRIGGER refuse_recovery BEFORE UPDATE OF status ON turn_runs
         WHEN NEW.agent_id='b' AND NEW.status='RUNNING'
         BEGIN SELECT RAISE(ABORT, 'recovery admission unavailable'); END;",
        )
        .unwrap();
    let before = snapshot(&ctl.store.conn);
    let ids = vec![first.run_id.clone(), second.run_id.clone()];
    let error = ctl.restore_queued_runs(&ids).unwrap_err();
    assert!(error.contains("recovery admission unavailable"), "{error}");
    assert_eq!(snapshot(&ctl.store.conn), before);
    ctl.store.conn.execute_batch("DROP TRIGGER refuse_recovery").unwrap();
    let events = ctl.store.events("s", 0, 1000).unwrap();
    let restored = ctl.restore_queued_runs(&ids).unwrap();
    assert_eq!(restored.len(), 2);
    for run in restored {
        assert_eq!(run.status, TurnStatus::Running);
        assert!(run.cancel_requested, "recovery must retain the user's stop request");
        let original = if run.run_id == first.run_id { &first } else { &second };
        assert_eq!(run.input_delivery_ids, original.input_delivery_ids);
    }
    assert_eq!(ctl.store.agent_status("s", "leader").unwrap(), Some(AgentStatus::Busy));
    assert_eq!(ctl.store.agent_status("s", "b").unwrap(), Some(AgentStatus::Draining));
    assert_eq!(ctl.store.events("s", 0, 1000).unwrap(), events);
    let admitted = snapshot(&ctl.store.conn);
    ctl.restore_queued_runs(&ids).unwrap();
    assert_eq!(snapshot(&ctl.store.conn), admitted, "repeated admission must not change a running turn");
}

#[test]
fn malformed_task_lists_cannot_skip_dependencies_or_become_empty_assignments() {
    for field in ["dependencies", "result_refs"] {
        for bad in ["[", "null", "{}", "[false]"] {
            let mut ctl = harness();
            seed_task(&ctl, "dependency", TaskStatus::Running, &[]);
            seed_task(&ctl, "work", TaskStatus::Pending, &["dependency"]);
            ctl.store.conn.execute(&format!("UPDATE tasks SET {field}=?1 WHERE task_id='work'"), [bad]).unwrap();
            let before = snapshot(&ctl.store.conn);
            let error = ctl.schedule().expect_err("corrupt task must stop scheduling");
            assert!(error.contains(field), "{error}");
            assert!(ctl.agent_view("b").is_err(), "a corrupt assignment must not disappear from the view");
            assert_eq!(snapshot(&ctl.store.conn), before);
            let repaired = if field == "dependencies" { r#"["dependency"]"# } else { "[]" };
            ctl.store.conn.execute(&format!("UPDATE tasks SET {field}=?1 WHERE task_id='work'"), [repaired]).unwrap();
            ctl.schedule().unwrap();
            assert!(ctl.store.runs_for_session("s", &[]).unwrap().is_empty());
            ctl.store.compare_and_set_task("dependency", "RUNNING", TaskStatus::Succeeded, None).unwrap();
            ctl.schedule().unwrap();
            assert!(ctl
                .store
                .runs_for_session("s", &[])
                .unwrap()
                .iter()
                .any(|run| run.task_id.as_deref() == Some("work")));
        }
    }
}

#[test]
fn malformed_run_lists_cannot_be_overwritten_when_inputs_arrive() {
    for field in ["input_delivery_ids", "waiting_on"] {
        for bad in ["[", "null", "{}", "[false]"] {
            let mut ctl = harness();
            let run = queued_leader(&mut ctl);
            let original: String =
                ctl.store.conn.query_row(&format!("SELECT {field} FROM turn_runs"), [], |row| row.get(0)).unwrap();
            ctl.store.conn.execute(&format!("UPDATE turn_runs SET {field}=?1"), [bad]).unwrap();
            let before = snapshot(&ctl.store.conn);
            let error = ctl.schedule().expect_err("corrupt run must not look empty");
            assert!(error.contains(field), "{error}");
            assert!(ctl.begin_run(&run.run_id).is_err());
            assert_eq!(snapshot(&ctl.store.conn), before);
            ctl.store.conn.execute(&format!("UPDATE turn_runs SET {field}=?1"), [original]).unwrap();
            ctl.schedule().unwrap();
            let resumed = ctl.begin_run(&run.run_id).unwrap();
            assert_eq!(resumed.input_delivery_ids, run.input_delivery_ids);
            assert_eq!(ctl.agent_view("leader").unwrap()["inbox_delta"][0]["payload"]["text"], "preserve input");
        }
    }
}

#[test]
fn malformed_pending_deliveries_are_preserved_instead_of_dropped() {
    for (table, field, bad) in [
        ("events", "payload_json", "{"),
        ("events", "payload_json", "null"),
        ("events", "payload_json", "[]"),
        ("events", "audience_json", "["),
        ("events", "audience_json", "[false]"),
        ("events", "kind", "future_event"),
        ("deliveries", "payload_override", "{"),
        ("deliveries", "payload_override", "null"),
        ("deliveries", "payload_override", "[]"),
    ] {
        let mut ctl = harness();
        let run = queued_leader(&mut ctl);
        let original: Option<String> =
            ctl.store.conn.query_row(&format!("SELECT {field} FROM {table} LIMIT 1"), [], |row| row.get(0)).unwrap();
        ctl.store.conn.execute(&format!("UPDATE {table} SET {field}=?1"), [bad]).unwrap();
        let before = snapshot(&ctl.store.conn);
        let error = ctl.agent_view("leader").expect_err("malformed delivery must not become a revocation");
        assert!(error.contains(field), "{field}: {error}");
        assert!(ctl.delivery_items("leader", &run.input_delivery_ids).is_err());
        assert!(ctl.schedule().is_err());
        assert_eq!(snapshot(&ctl.store.conn), before, "{table}.{field}");
        ctl.store.conn.execute(&format!("UPDATE {table} SET {field}=?1"), [original]).unwrap();
        assert_eq!(ctl.delivery_items("leader", &run.input_delivery_ids).unwrap().len(), 1);
        assert_eq!(ctl.agent_view("leader").unwrap()["inbox_delta"][0]["payload"]["text"], "preserve input");
    }
}

#[test]
fn run_preparation_rolls_back_start_events_and_keeps_the_original_intent() {
    let mut ctl = harness();
    let run = queued_leader(&mut ctl);
    ctl.store.conn.execute("INSERT INTO shared_cursors VALUES('s','leader','main','broken')", []).unwrap();
    let before = snapshot(&ctl.store.conn);
    assert!(ctl.prepare_run(&run.run_id).is_err());
    assert_eq!(snapshot(&ctl.store.conn), before);
    ctl.store.conn.execute("UPDATE shared_cursors SET sequence=0", []).unwrap();
    let prepared = ctl.prepare_run(&run.run_id).unwrap();
    assert_eq!(prepared["run"]["run_id"], run.run_id);
    assert_eq!(prepared["run"]["status"], "RUNNING");
    assert_eq!(prepared["view"]["inbox_delta"][0]["payload"]["text"], "preserve input");
    assert_eq!(prepared["wake"]["reason"], "user_input");
    assert_eq!(ctl.store.events("s", 0, 100).unwrap().iter().filter(|event| event["kind"] == "run_started").count(), 1);
}

#[test]
fn unreadable_view_metadata_rolls_back_delivery_projection() {
    for fault in ["cursor", "member", "shared_table"] {
        let mut ctl = harness();
        queued_leader(&mut ctl);
        match fault {
            "cursor" => {
                ctl.store.conn.execute("INSERT INTO shared_cursors VALUES('s','leader','main','broken')", []).unwrap();
            }
            "member" => {
                ctl.store.conn.execute("UPDATE agent_runtime SET status='broken' WHERE agent_id='b'", []).unwrap();
            }
            _ => {
                ctl.store
                    .conn
                    .execute_batch("ALTER TABLE shared_entries RENAME TO unavailable_shared_entries")
                    .unwrap();
            }
        }
        let before = snapshot(&ctl.store.conn);
        assert!(ctl.agent_view("leader").is_err(), "{fault}: missing data was hidden");
        assert_eq!(snapshot(&ctl.store.conn), before, "{fault}: a failed view changed the input ledger");
        match fault {
            "cursor" => {
                ctl.store.conn.execute("UPDATE shared_cursors SET sequence=0", []).unwrap();
            }
            "member" => {
                ctl.store.set_agent_status("s", "b", AgentStatus::Idle).unwrap();
            }
            _ => {
                ctl.store
                    .conn
                    .execute_batch("ALTER TABLE unavailable_shared_entries RENAME TO shared_entries")
                    .unwrap();
            }
        }
        assert_eq!(ctl.agent_view("leader").unwrap()["inbox_delta"].as_array().unwrap().len(), 1);
    }
}

#[test]
fn corrupt_approval_scope_prevents_finalization_and_preserves_the_decision() {
    let mut ctl = harness();
    let queued = queued_leader(&mut ctl);
    let run = ctl.begin_run(&queued.run_id).unwrap();
    approval(&ctl, &run, "first", ApprovalStatus::Pending, 1.0);
    ctl.store.conn.execute("UPDATE approvals SET requested_scope='{'", []).unwrap();
    let before = snapshot(&ctl.store.conn);
    let error = ctl
        .finalize_run(&run.run_id, &outcome(), &run.input_delivery_ids)
        .expect_err("invalid approval must not be ignored");
    assert!(error.contains("requested_scope"), "{error}");
    assert_eq!(snapshot(&ctl.store.conn), before);
    ctl.store.conn.execute("UPDATE approvals SET requested_scope='{}'", []).unwrap();
    ctl.finalize_run(&run.run_id, &outcome(), &run.input_delivery_ids).unwrap();
    assert_eq!(ctl.store.get_approval("first").unwrap().unwrap().status, ApprovalStatus::Expired);
}

#[test]
fn approval_expiry_failure_rolls_back_finalization_timeout_and_parked_cancel() {
    for operation in ["finalize", "timeout", "parked"] {
        let mut ctl = harness();
        let queued = queued_leader(&mut ctl);
        let run = ctl.begin_run(&queued.run_id).unwrap();
        approval(&ctl, &run, "first", ApprovalStatus::Pending, 1.0);
        approval(&ctl, &run, "second", ApprovalStatus::Pending, 2.0);
        if operation == "parked" {
            ctl.store.conn.execute("UPDATE turn_runs SET status='WAITING_APPROVAL',cancel_requested=1", []).unwrap();
        }
        ctl.store
            .conn
            .execute_batch(
                "CREATE TRIGGER refuse_expiry BEFORE UPDATE ON approvals
             WHEN NEW.approval_id='second' AND NEW.status='EXPIRED'
             BEGIN SELECT RAISE(ABORT, 'expiry unavailable'); END;",
            )
            .unwrap();
        let before = snapshot(&ctl.store.conn);
        let perform = |ctl: &mut Control| match operation {
            "finalize" => ctl.finalize_run(&run.run_id, &outcome(), &run.input_delivery_ids).map(|_| ()),
            "timeout" => ctl.stop_timeout(&run.run_id),
            _ => ctl.schedule(),
        };
        let error = perform(&mut ctl).expect_err("expiry failure must abort the enclosing transition");
        assert!(error.contains("expiry unavailable"), "{error}");
        assert_eq!(snapshot(&ctl.store.conn), before, "{operation}: partial transition committed");
        ctl.store.conn.execute_batch("DROP TRIGGER refuse_expiry").unwrap();
        perform(&mut ctl).unwrap();
        for id in ["first", "second"] {
            assert_eq!(ctl.store.get_approval(id).unwrap().unwrap().status, ApprovalStatus::Expired);
        }
        assert_eq!(
            ctl.store.events("s", 0, 100).unwrap().iter().filter(|e| e["kind"] == "approval_decided").count(),
            2
        );
    }
}

#[test]
fn corrupt_wake_results_cannot_start_a_turn_with_empty_task_results() {
    let mut ctl = harness();
    seed_task(&ctl, "finished", TaskStatus::Succeeded, &[]);
    let run: TurnRun = serde_json::from_value(json!({
        "run_id":"waiter","session_id":"s","agent_id":"leader",
        "config_revision":1,"topology_revision":1,"waiting_on":["finished"]
    }))
    .unwrap();
    ctl.store.insert_run(&run).unwrap();
    ctl.store.conn.execute("UPDATE tasks SET result_refs='['", []).unwrap();
    let before = snapshot(&ctl.store.conn);
    let error = ctl.begin_run(&run.run_id).expect_err("corrupt wake results must not look successful");
    assert!(error.contains("result_refs"), "{error}");
    assert_eq!(snapshot(&ctl.store.conn), before);
    ctl.store.conn.execute("UPDATE tasks SET result_refs='[]'", []).unwrap();
    ctl.begin_run(&run.run_id).unwrap();
    assert_eq!(ctl.store.get_run(&run.run_id).unwrap().unwrap().status, TurnStatus::Running);
}

#[test]
fn corrupt_session_approval_cache_is_not_reported_as_a_missing_grant() {
    let ctl = harness();
    ctl.store.cache_session_approval("s", "hash", &json!({"tool":"shell"})).unwrap();
    ctl.store.conn.execute("UPDATE session_approval_cache SET scope_json='{'", []).unwrap();
    let before = snapshot(&ctl.store.conn);
    let error = ctl.store.find_session_approval("s", "hash").expect_err("a broken grant is not an absent grant");
    assert!(error.to_string().contains("scope_json"), "{error}");
    assert_eq!(snapshot(&ctl.store.conn), before);
    assert!(ctl.store.find_session_approval("s", "other").unwrap().is_none());
    ctl.store.conn.execute("UPDATE session_approval_cache SET scope_json='null'", []).unwrap();
    assert_eq!(ctl.store.find_session_approval("s", "hash").unwrap(), Some(Json::Null));
}

#[test]
fn unreadable_terminal_dependency_cannot_silently_stall_scheduling() {
    let mut ctl = harness();
    seed_task(&ctl, "finished", TaskStatus::Succeeded, &[]);
    seed_task(&ctl, "work", TaskStatus::Pending, &["finished"]);
    ctl.store.conn.execute("UPDATE tasks SET result_refs='[true]' WHERE task_id='finished'", []).unwrap();
    let before = snapshot(&ctl.store.conn);
    let error = ctl.schedule().expect_err("a corrupt dependency must not look unfinished or absent");
    assert!(error.contains("result_refs"), "{error}");
    assert_eq!(snapshot(&ctl.store.conn), before);
    ctl.store.conn.execute("UPDATE tasks SET result_refs='[]' WHERE task_id='finished'", []).unwrap();
    ctl.schedule().unwrap();
    assert!(ctl.store.runs_for_session("s", &[]).unwrap().iter().any(|run| run.task_id.as_deref() == Some("work")));
}

#[test]
fn unreadable_decided_approval_cannot_start_as_an_unrelated_wake() {
    let mut ctl = harness();
    let run = queued_leader(&mut ctl);
    approval(&ctl, &run, "decision", ApprovalStatus::Denied, 1.0);
    ctl.store.conn.execute("UPDATE approvals SET requested_scope='{'", []).unwrap();
    let before = snapshot(&ctl.store.conn);
    let error = ctl.begin_run(&run.run_id).expect_err("a stored decision must not silently disappear");
    assert!(error.contains("requested_scope"), "{error}");
    assert_eq!(snapshot(&ctl.store.conn), before);
    ctl.store.conn.execute("UPDATE approvals SET requested_scope='null'", []).unwrap();
    let resumed = ctl.begin_run(&run.run_id).unwrap();
    let wake = ctl.wake_info(&resumed).unwrap();
    assert_eq!(wake["reason"], "approval");
    assert_eq!(wake["payload"]["denied"], true);
}

#[test]
fn cancelling_a_queued_run_does_not_require_its_view_or_replay_its_inputs() {
    for paused in [false, true] {
        let mut ctl = harness();
        let run = queued_leader(&mut ctl);
        ctl.store.conn.execute("INSERT INTO shared_cursors VALUES('s','leader','main','broken')", []).unwrap();
        assert!(ctl.prepare_run(&run.run_id).is_err());
        if paused {
            assert!(ctl.submit(&action("pause", "user", ActionKind::PauseSession, json!({}))).unwrap().ok);
        }
        let cancel = action("cancel", "user", ActionKind::CancelRun, json!({"run_id":run.run_id}));
        let receipt = ctl.submit(&cancel).unwrap();
        assert!(receipt.ok, "{receipt:?}");
        assert_eq!(ctl.store.get_run(&run.run_id).unwrap().unwrap().status, TurnStatus::Cancelled);
        assert!(ctl.prepare_run(&run.run_id).is_err());
        ctl.schedule().unwrap();
        assert_eq!(ctl.store.runs_for_session("s", &[]).unwrap().len(), 1, "cancelled input must not restart");
        assert!(ctl.store.pending_deliveries("s", "leader").unwrap().is_empty());
        assert_eq!(ctl.store.applied_batch("s", "leader").unwrap(), 0, "unread input is not consumed");
        let (status, reason): (String, String) = ctl
            .store
            .conn
            .query_row(
                "SELECT status,payload_override FROM deliveries WHERE delivery_id=?1",
                [run.input_delivery_ids[0]],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "dropped");
        assert!(serde_json::from_str::<Json>(&reason).unwrap()["dropped_reason"].as_str().unwrap().contains("cancel"));
        let events = ctl.store.events("s", 0, 100).unwrap();
        assert_eq!(events.iter().filter(|event| event["kind"] == "run_started").count(), 0);
        assert_eq!(events.iter().filter(|event| event["kind"] == "run_cancelled").count(), 1);
        let before_replay = snapshot(&ctl.store.conn);
        assert_eq!(serde_json::to_value(ctl.submit(&cancel).unwrap()).unwrap(), serde_json::to_value(receipt).unwrap());
        assert_eq!(snapshot(&ctl.store.conn), before_replay);
    }
}

#[test]
fn queued_cancellation_failure_keeps_the_run_approvals_and_input_together() {
    for (kind, attached_task) in
        [(ActionKind::CancelRun, false), (ActionKind::CancelRun, true), (ActionKind::CancelTask, true)]
    {
        let mut ctl = harness();
        let run = if attached_task {
            assert!(
                ctl.submit(&action(
                    "assign",
                    "leader",
                    ActionKind::AssignTask,
                    json!({"assignee":"b","description":"work"})
                ))
                .unwrap()
                .ok
            );
            ctl.store.runs_for_session("s", &[TurnStatus::Queued]).unwrap().remove(0)
        } else {
            queued_leader(&mut ctl)
        };
        approval(&ctl, &run, "pending", ApprovalStatus::Pending, 1.0);
        ctl.store
            .conn
            .execute_batch(
                "CREATE TRIGGER refuse_cancel_input BEFORE UPDATE OF status ON deliveries
             WHEN NEW.status='dropped'
             BEGIN SELECT RAISE(ABORT, 'cancel input storage unavailable'); END;",
            )
            .unwrap();
        let payload = match kind {
            ActionKind::CancelTask => json!({"task_id":run.task_id}),
            _ => json!({"run_id":run.run_id}),
        };
        let mut before = snapshot(&ctl.store.conn);
        before.as_object_mut().unwrap().remove("actions");
        let cancel = action("cancel-fails", "user", kind, payload.clone());
        let receipt = ctl.submit(&cancel).unwrap();
        assert!(!receipt.ok, "uncommitted input disposition must reject the cancellation");
        assert!(receipt.error.as_deref().unwrap().contains("cancel input storage unavailable"));
        let mut after = snapshot(&ctl.store.conn);
        after.as_object_mut().unwrap().remove("actions");
        assert_eq!(after, before, "only the refusal receipt may commit");
        ctl.store.conn.execute_batch("DROP TRIGGER refuse_cancel_input").unwrap();
        assert!(ctl.submit(&action("cancel-retry", "user", kind, payload)).unwrap().ok);
        assert_eq!(ctl.store.get_run(&run.run_id).unwrap().unwrap().status, TurnStatus::Cancelled);
        assert_eq!(ctl.store.get_approval("pending").unwrap().unwrap().status, ApprovalStatus::Expired);
        assert!(ctl.store.pending_deliveries("s", &run.agent_id).unwrap().is_empty());
        if let Some(task_id) = &run.task_id {
            assert_eq!(ctl.store.get_task(task_id).unwrap().unwrap().status, TaskStatus::Cancelled);
        }
        assert_eq!(
            ctl.store.runs_for_session("s", &[]).unwrap().iter().filter(|other| other.agent_id == run.agent_id).count(),
            1
        );
    }
}

#[test]
fn scheduling_converges_an_older_queued_cancel_request_without_preparing_its_view() {
    let mut ctl = harness();
    let run = queued_leader(&mut ctl);
    approval(&ctl, &run, "pending", ApprovalStatus::Pending, 1.0);
    ctl.store.conn.execute("INSERT INTO shared_cursors VALUES('s','leader','main','broken')", []).unwrap();
    ctl.store.set_run_cancel_requested(&run.run_id).unwrap();
    for _ in 0..2 {
        ctl.schedule().unwrap();
    }
    assert_eq!(ctl.store.get_run(&run.run_id).unwrap().unwrap().status, TurnStatus::Cancelled);
    assert!(ctl.store.pending_deliveries("s", "leader").unwrap().is_empty());
    assert_eq!(ctl.store.applied_batch("s", "leader").unwrap(), 0);
    assert_eq!(ctl.store.get_approval("pending").unwrap().unwrap().status, ApprovalStatus::Expired);
    assert_eq!(ctl.store.runs_for_session("s", &[]).unwrap().len(), 1);
    let events = ctl.store.events("s", 0, 100).unwrap();
    assert!(!events.iter().any(|event| event["kind"] == "run_started"));
    assert_eq!(events.iter().filter(|event| event["kind"] == "run_cancelled").count(), 1);
}
