//! Communication and local control requests must preserve intent and read cursors.

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
        "channels":[
            {"source":"leader","targets":["b"],"mode":"task"},
            {"source":"b","targets":["leader"],"mode":"message"}
        ],
        "shared_spaces":[
            {"id":"one","readers":["leader","b"],"writers":["leader","b"]},
            {"id":"two","readers":["leader","b"],"writers":["leader","b"]},
            {"id":"secret","readers":["leader"],"writers":["leader"]}
        ]
    }))
    .unwrap();
    for session in ["s1", "s2"] {
        store.create_session(session, "/tmp", "approved_scope").unwrap();
        store.save_team_spec(session, &spec).unwrap();
        for member in &spec.agents {
            store.ensure_agent(session, &member.id).unwrap();
        }
    }
    Control::new(store, "s1")
}

fn action(id: &str, actor: &str, kind: ActionKind, payload: Json) -> TeamAction {
    TeamAction { action_id: id.into(), session_id: "s1".into(), actor_id: actor.into(), kind, payload, run_id: None }
}

fn seed(ctl: &Control, session: &str, space: &str, id: &str) -> i64 {
    let entry: SharedEntry = serde_json::from_value(json!({
        "entry_id":id,"space_id":space,"author":"leader","content":id,"created_at":1234.5
    }))
    .unwrap();
    ctl.store.add_shared_entry(&entry, session).unwrap()
}

fn business(ctl: &Control) -> Json {
    let spaces = vec!["one".into(), "two".into(), "secret".into()];
    json!(["s1", "s2"].map(|session| json!({
        "session":ctl.store.get_session(session).unwrap(),
        "events":ctl.store.events(session, 0, 1000).unwrap(),
        "shared":ctl.store.shared_entries(session, &spaces, 0, 2000).unwrap(),
        "tasks":ctl.store.tasks_for_session(session, &[]).unwrap(),
        "members":(["leader","b"].map(|agent| json!({
            "status":ctl.store.agent_status(session,agent).unwrap(),
            "deliveries":ctl.store.pending_deliveries(session,agent).unwrap(),
            "cursors":spaces.iter().map(|space| ctl.store.shared_cursor(session,agent,space).unwrap()).collect::<Vec<_>>()
        })))
    })))
}

fn refused_unchanged(ctl: &mut Control, request: &TeamAction) {
    let before = business(ctl);
    let receipt = ctl.submit(request).unwrap();
    assert!(!receipt.ok, "accepted malformed request {request:?}: {receipt:?}");
    assert!(receipt.error.as_ref().is_some_and(|error| !error.is_empty()));
    assert_eq!(business(ctl), before, "a refusal changed business state");
    assert_eq!(json!(ctl.submit(request).unwrap()), json!(receipt), "refusal replay changed");
    assert_eq!(business(ctl), before);
}

fn start_leader(ctl: &mut Control) -> TurnRun {
    assert!(ctl.submit(&action("start", "user", ActionKind::UserMessage, json!({"text":"work"}))).unwrap().ok);
    let queued = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    ctl.begin_run(&queued.run_id).unwrap()
}

#[test]
fn communication_requests_refuse_wrong_types_and_unknown_fields_without_delivery() {
    let mut ctl = harness();
    for (index, (kind, payload)) in [
        (ActionKind::SendMessage, json!({"target":"leader","text":false})),
        (ActionKind::SendMessage, json!({"target":"leader"})),
        (ActionKind::SendMessage, json!({"target":"leader","text":"ok","actor_id":"leader"})),
        (ActionKind::PublishShared, json!({"space_id":"one","content":{"private":"must not stringify"}})),
        (ActionKind::PublishShared, json!({"space_id":"one","content":"ok","kind":true})),
        (ActionKind::PublishShared, json!({"space_id":"one","content":null,"ref":"report.md"})),
        (ActionKind::PublishShared, json!({"space_id":"one","content":"ok","unknown":1})),
        (ActionKind::RequestHelp, json!({"message":123})),
        (ActionKind::RequestHelp, json!({"message":"help","task_id":false})),
        (ActionKind::RequestHelp, json!({"message":"help","target":"other"})),
        (ActionKind::ListShared, json!({"space_id":"one"})),
        (ActionKind::ListShared, json!([])),
    ]
    .into_iter()
    .enumerate()
    {
        refused_unchanged(&mut ctl, &action(&format!("bad-{index}"), "b", kind, payload));
    }
    for (index, (kind, payload)) in [
        (ActionKind::SendMessage, json!({"target":"leader","text":"corrected"})),
        (ActionKind::PublishShared, json!({"space_id":"one","content":"corrected"})),
        (ActionKind::RequestHelp, json!({"message":"corrected"})),
        (ActionKind::ListShared, json!({})),
    ]
    .into_iter()
    .enumerate()
    {
        let receipt = ctl.submit(&action(&format!("good-{index}"), "b", kind, payload)).unwrap();
        assert!(receipt.ok, "{receipt:?}");
    }
    assert_eq!(ctl.store.shared_entries("s1", &["one".into()], 0, 10).unwrap().len(), 1);
    assert_eq!(ctl.store.events("s1", 0, 100).unwrap().iter().filter(|e| e["kind"] == "message").count(), 2);
}

#[test]
fn malformed_user_input_cannot_resume_a_paused_session_or_clear_completion() {
    let mut ctl = harness();
    let run = start_leader(&mut ctl);
    ctl.store.record_completion_request(&run.run_id, "", &[], "ready").unwrap();
    assert!(ctl.submit(&action("pause", "user", ActionKind::PauseSession, json!({}))).unwrap().ok);
    for (index, kind) in [ActionKind::UserMessage, ActionKind::UserSupplement].into_iter().enumerate() {
        for (suffix, payload) in [("type", json!({"text":true})), ("extra", json!({"text":"ok","unknown":1}))] {
            refused_unchanged(&mut ctl, &action(&format!("bad-{index}-{suffix}"), "user", kind, payload));
            assert_eq!(ctl.store.completion_request(&run.run_id).unwrap().unwrap()["summary"], "ready");
        }
    }
    assert!(
        ctl.submit(&action("continue", "user", ActionKind::UserSupplement, json!({"text":"continue"}))).unwrap().ok
    );
    assert_eq!(ctl.store.get_session("s1").unwrap().unwrap()["status"], "ACTIVE");
    assert!(ctl.store.completion_request(&run.run_id).unwrap().is_none());
}

#[test]
fn control_requests_refuse_ignored_options_before_pausing_or_changing_permissions() {
    let mut ctl = harness();
    for (index, (kind, payload)) in [
        (ActionKind::PauseSession, json!({"paused":false})),
        (ActionKind::PauseSession, json!(null)),
        (ActionKind::PauseSession, json!([])),
        (ActionKind::SetPermissionMode, json!({"mode":"full_auto","scope":"read-only"})),
        (ActionKind::SetPermissionMode, json!({"mode":false})),
    ]
    .into_iter()
    .enumerate()
    {
        refused_unchanged(&mut ctl, &action(&format!("bad-{index}"), "user", kind, payload));
    }
    assert!(
        ctl.submit(&action("mode", "user", ActionKind::SetPermissionMode, json!({"mode":"full_auto"}))).unwrap().ok
    );
    assert!(ctl.submit(&action("pause", "user", ActionKind::PauseSession, json!({}))).unwrap().ok);
    let session = ctl.store.get_session("s1").unwrap().unwrap();
    assert_eq!(session["status"], "PAUSED");
    assert_eq!(session["permissions_mode"], "full_auto");
}

#[test]
fn completion_request_rejects_bad_summary_and_non_object_payloads() {
    let mut ctl = harness();
    let run = start_leader(&mut ctl);
    for (index, payload) in
        [json!({"summary":false}), json!({"summary":"ok","force":true}), json!([])].into_iter().enumerate()
    {
        let mut request = action(&format!("bad-{index}"), "leader", ActionKind::SignalDone, payload);
        request.run_id = Some(run.run_id.clone());
        refused_unchanged(&mut ctl, &request);
        assert!(ctl.store.completion_request(&run.run_id).unwrap().is_none());
    }
    let mut request = action("done", "leader", ActionKind::SignalDone, json!({"summary":"verified"}));
    request.run_id = Some(run.run_id.clone());
    assert!(ctl.submit(&request).unwrap().ok);
    ctl.finalize_run(
        &run.run_id,
        &TurnOutcome { status: TurnStatus::Completed, error: None, note: None, reply_text: None },
        &run.input_delivery_ids,
    )
    .unwrap();
    assert_eq!(ctl.store.events("s1", 0, 100).unwrap().iter().filter(|e| e["kind"] == "goal_done").count(), 1);
}

#[test]
fn shared_read_invalid_scope_or_cursor_cannot_consume_other_spaces() {
    let mut ctl = harness();
    seed(&ctl, "s1", "one", "first");
    seed(&ctl, "s1", "two", "second");
    seed(&ctl, "s1", "secret", "PRIVATE");
    for (index, payload) in [
        json!({"space_id":false}),
        json!({"space_id":null}),
        json!({"space_id":["one"]}),
        json!({"space_id":"one","after_sequence":true}),
        json!({"space_id":"one","after_sequence":"0"}),
        json!({"space_id":"one","after_sequence":0.5}),
        json!({"space_id":"one","after_sequence":-1}),
        json!({"space_id":"one","after_sequence":null}),
        json!({"space_id":"one","limit":false}),
        json!({"space_id":"one","limit":"1"}),
        json!({"space_id":"one","limit":1.5}),
        json!({"space_id":"one","unknown":1}),
    ]
    .into_iter()
    .enumerate()
    {
        refused_unchanged(&mut ctl, &action(&format!("bad-{index}"), "b", ActionKind::ReadShared, payload));
    }
    let receipt =
        ctl.submit(&action("read", "b", ActionKind::ReadShared, json!({"space_id":"one","limit":1}))).unwrap();
    assert!(receipt.ok, "{receipt:?}");
    assert_eq!(receipt.result["entries"][0]["content"], "first");
    assert_eq!(ctl.store.shared_cursor("s1", "b", "two").unwrap(), 0);
    assert!(!receipt.result.to_string().contains("PRIVATE"));
}

#[test]
fn shared_read_without_explicit_cursor_respects_each_space_and_page_limit() {
    let mut ctl = harness();
    let first = seed(&ctl, "s1", "one", "one-old");
    seed(&ctl, "s1", "two", "two-new");
    seed(&ctl, "s1", "one", "one-new");
    seed(&ctl, "s2", "one", "FOREIGN");
    seed(&ctl, "s1", "secret", "PRIVATE");
    ctl.store.advance_shared_cursor("s1", "b", "one", first).unwrap();
    for (index, expected) in ["two-new", "one-new"].into_iter().enumerate() {
        let receipt =
            ctl.submit(&action(&format!("page-{index}"), "b", ActionKind::ReadShared, json!({"limit":1}))).unwrap();
        assert!(receipt.ok, "{receipt:?}");
        assert_eq!(receipt.result["entries"].as_array().unwrap().len(), 1);
        assert_eq!(receipt.result["entries"][0]["content"], expected);
    }
    let empty = ctl.submit(&action("empty", "b", ActionKind::ReadShared, json!({}))).unwrap();
    assert!(empty.ok);
    assert_eq!(empty.result["entries"], json!([]));
    let reread = ctl
        .submit(&action("reread", "b", ActionKind::ReadShared, json!({"space_id":"one","after_sequence":0})))
        .unwrap();
    assert!(reread.ok);
    assert_eq!(reread.result["entries"].as_array().unwrap().len(), 2);
}

#[test]
fn shared_listing_reports_exact_counts_and_latest_sequence_beyond_one_thousand() {
    let mut ctl = harness();
    let mut last = 0;
    for index in 0..1002 {
        last = seed(&ctl, "s1", "one", &format!("entry-{index}"));
    }
    seed(&ctl, "s1", "secret", "PRIVATE");
    seed(&ctl, "s2", "one", "FOREIGN");
    let receipt = ctl.submit(&action("list", "b", ActionKind::ListShared, json!({}))).unwrap();
    assert!(receipt.ok, "{receipt:?}");
    assert_eq!(receipt.result["spaces"].as_array().unwrap().len(), 2);
    let first = receipt.result["spaces"].as_array().unwrap().iter().find(|space| space["space_id"] == "one").unwrap();
    assert_eq!(first["entries"], 1002);
    assert_eq!(first["last_sequence"], last);
    assert_eq!(ctl.store.shared_cursor("s1", "b", "one").unwrap(), 0);
}

#[test]
fn shared_replacement_and_help_references_are_checked_in_the_current_session() {
    let mut ctl = harness();
    seed(&ctl, "s1", "one", "old");
    seed(&ctl, "s2", "one", "foreign");
    seed(&ctl, "s1", "secret", "private");
    for (index, reference) in ["missing", "foreign", "private"].into_iter().enumerate() {
        refused_unchanged(
            &mut ctl,
            &action(
                &format!("replace-{index}"),
                "b",
                ActionKind::PublishShared,
                json!({"space_id":"one","content":"updated","supersedes":reference}),
            ),
        );
    }
    let receipt = ctl
        .submit(&action(
            "replace",
            "b",
            ActionKind::PublishShared,
            json!({"space_id":"one","content":"updated","supersedes":"old"}),
        ))
        .unwrap();
    assert!(receipt.ok, "{receipt:?}");
    let entries = ctl.store.shared_entries("s1", &["one".into()], 0, 10).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].content, "old");
    assert_eq!(entries[1].supersedes.as_deref(), Some("old"));

    let task: Task = serde_json::from_value(json!({
        "task_id":"foreign-task","requester":"leader","assignee":"b","description":"FOREIGN"
    }))
    .unwrap();
    ctl.store.insert_task("s2", &task).unwrap();
    for (index, id) in ["missing", "foreign-task"].into_iter().enumerate() {
        refused_unchanged(
            &mut ctl,
            &action(&format!("help-{index}"), "b", ActionKind::RequestHelp, json!({"message":"help","task_id":id})),
        );
    }
    let own = ctl
        .submit(&action("assign", "leader", ActionKind::AssignTask, json!({"assignee":"b","description":"work"})))
        .unwrap();
    assert!(own.ok, "{own:?}");
    let receipt = ctl
        .submit(&action(
            "help",
            "b",
            ActionKind::RequestHelp,
            json!({"message":"help","task_id":own.result["task_id"]}),
        ))
        .unwrap();
    assert!(receipt.ok, "{receipt:?}");
}

#[test]
fn approval_decisions_refuse_unknown_fields_before_granting_or_waking() {
    for (decision, status) in [
        ("once", ApprovalStatus::ApprovedOnce),
        ("session", ApprovalStatus::ApprovedSession),
        ("deny", ApprovalStatus::Denied),
    ] {
        let mut ctl = harness();
        let run = start_leader(&mut ctl);
        ctl.store.set_run_status(&run.run_id, TurnStatus::WaitingApproval).unwrap();
        ctl.store.set_agent_status("s1", "leader", AgentStatus::Waiting).unwrap();
        let approval = ApprovalRequest {
            approval_id: "approval".into(),
            session_id: "s1".into(),
            agent_id: "leader".into(),
            run_id: run.run_id.clone(),
            tool_call_id: "call".into(),
            operation_hash: "hash".into(),
            requested_scope: json!({"path":"report.md"}),
            policy_revision: 1,
            status: ApprovalStatus::Pending,
            created_at: now(),
            decided_at: None,
        };
        ctl.store.insert_approval(&approval).unwrap();
        refused_unchanged(
            &mut ctl,
            &action(
                "bad",
                "user",
                ActionKind::ApprovalDecision,
                json!({"approval_id":"approval","decision":decision,"scope":"read-only"}),
            ),
        );
        assert_eq!(json!(ctl.store.get_approval_for_session("s1", "approval").unwrap()), json!(Some(&approval)));
        assert!(ctl.store.find_session_approval("s1", "hash").unwrap().is_none());
        assert_eq!(ctl.store.get_run(&run.run_id).unwrap().unwrap().status, TurnStatus::WaitingApproval);
        let receipt = ctl
            .submit(&action(
                "good",
                "user",
                ActionKind::ApprovalDecision,
                json!({"approval_id":"approval","decision":decision}),
            ))
            .unwrap();
        assert!(receipt.ok, "{receipt:?}");
        assert_eq!(ctl.store.get_approval_for_session("s1", "approval").unwrap().unwrap().status, status);
        assert_eq!(ctl.store.find_session_approval("s1", "hash").unwrap().is_some(), decision == "session");
    }
}

#[test]
fn action_payloads_require_objects_even_when_serde_can_decode_sequences() {
    let mut ctl = harness();
    refused_unchanged(&mut ctl, &action("assign-array", "leader", ActionKind::AssignTask, json!(["b", "work"])));
    let assigned = ctl
        .submit(&action("assign", "leader", ActionKind::AssignTask, json!({"assignee":"b","description":"work"})))
        .unwrap();
    assert!(assigned.ok, "{assigned:?}");
    let queued = ctl
        .store
        .runs_for_session("s1", &[TurnStatus::Queued])
        .unwrap()
        .into_iter()
        .find(|r| r.agent_id == "b")
        .unwrap();
    let run = ctl.begin_run(&queued.run_id).unwrap();
    let mut request = action(
        "complete-array",
        "b",
        ActionKind::CompleteTask,
        json!([assigned.result["task_id"], [], "should not complete"]),
    );
    request.run_id = Some(run.run_id.clone());
    refused_unchanged(&mut ctl, &request);
    assert!(ctl.store.completion_request(&run.run_id).unwrap().is_none());
}

#[test]
fn shared_cursor_read_and_write_failures_preserve_progress() {
    let mut ctl = harness();
    let first = seed(&ctl, "s1", "one", "first");
    seed(&ctl, "s1", "two", "second");
    ctl.store
        .conn
        .execute_batch(
            "CREATE TRIGGER refuse_cursor BEFORE INSERT ON shared_cursors WHEN NEW.space_id='two'
         BEGIN SELECT RAISE(ABORT,'cursor unavailable'); END;",
        )
        .unwrap();
    let read = action("failed-read", "b", ActionKind::ReadShared, json!({}));
    refused_unchanged(&mut ctl, &read);
    assert_eq!(ctl.store.shared_cursor("s1", "b", "one").unwrap(), 0);
    assert_eq!(ctl.store.shared_cursor("s1", "b", "two").unwrap(), 0);
    ctl.store.conn.execute_batch("DROP TRIGGER refuse_cursor").unwrap();
    assert!(!ctl.submit(&read).unwrap().ok, "a saved failure must replay after repair");
    let receipt = ctl.submit(&action("read", "b", ActionKind::ReadShared, json!({}))).unwrap();
    assert!(receipt.ok, "{receipt:?}");
    assert_eq!(receipt.result["entries"].as_array().unwrap().len(), 2);
    ctl.store
        .conn
        .execute_batch(
            "UPDATE shared_cursors SET sequence='broken' WHERE session_id='s1' AND agent_id='b' AND space_id='one'",
        )
        .unwrap();
    let receipt = ctl.submit(&action("broken", "b", ActionKind::ReadShared, json!({}))).unwrap();
    assert!(!receipt.ok, "a corrupt cursor must not become an empty page or restart at zero: {receipt:?}");
    let raw: String = ctl
        .store
        .conn
        .query_row(
            "SELECT sequence FROM shared_cursors WHERE session_id='s1' AND agent_id='b' AND space_id='one'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(raw, "broken");
    ctl.store
        .conn
        .execute(
            "UPDATE shared_cursors SET sequence=?1 WHERE session_id='s1' AND agent_id='b' AND space_id='one'",
            [first],
        )
        .unwrap();
    let receipt = ctl.submit(&action("repaired", "b", ActionKind::ReadShared, json!({}))).unwrap();
    assert!(receipt.ok, "{receipt:?}");
    assert_eq!(receipt.result["entries"], json!([]));
}

#[test]
fn shared_reads_and_refusals_replay_after_database_reopen() {
    struct Directory(std::path::PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let root = Directory(std::env::temp_dir().join(format!("ta-shared-reopen-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(&root.0).unwrap();
    let database = root.0.join("team.db");
    let mut ctl = harness();
    let first = seed(&ctl, "s1", "one", "first");
    seed(&ctl, "s1", "two", "second");
    let read = action("read", "b", ActionKind::ReadShared, json!({"limit":1}));
    let page = ctl.submit(&read).unwrap();
    assert!(page.ok);
    let bad = action(
        "bad",
        "b",
        ActionKind::PublishShared,
        json!({"space_id":"one","content":"updated","supersedes":"later"}),
    );
    let refusal = ctl.submit(&bad).unwrap();
    assert!(!refusal.ok, "{refusal:?}");
    seed(&ctl, "s1", "one", "later");
    ctl.store.conn.execute("VACUUM INTO ?1", [database.to_str().unwrap()]).unwrap();
    drop(ctl);
    let mut ctl = Control::new(Store::open(&database).unwrap(), "s1");
    let before = business(&ctl);
    assert_eq!(json!(ctl.submit(&read).unwrap()), json!(page));
    assert_eq!(json!(ctl.submit(&bad).unwrap()), json!(refusal));
    assert_eq!(business(&ctl), before);
    let mut collision = read.clone();
    collision.payload["limit"] = json!(2);
    assert!(ctl.submit(&collision).is_err());
    assert_eq!(ctl.store.shared_cursor("s1", "b", "one").unwrap(), first);
    assert_eq!(ctl.store.shared_cursor("s1", "b", "two").unwrap(), 0);
    let page = ctl.submit(&action("next", "b", ActionKind::ReadShared, json!({"limit":1}))).unwrap();
    assert!(page.ok);
    assert_eq!(page.result["entries"][0]["content"], "second");
    let mut corrected = bad;
    corrected.action_id = "corrected".into();
    assert!(ctl.submit(&corrected).unwrap().ok);
}
