//! Shared entries and task completion must not publish private context handles.

use serde_json::{json, Value as Json};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use teamagents_core::control::{Control, TurnOutcome};
use teamagents_core::models::*;
use teamagents_core::storage::Store;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("ta-output-refs-{}", uuid::Uuid::new_v4()));
        for path in ["project", "state/sessions/s1/artifacts", "state/sessions/s1/members/b/work"] {
            std::fs::create_dir_all(root.join(path)).unwrap();
        }
        std::fs::write(root.join("state/sessions/s1/members/b/chat_tree.json"), "PRIVATE_HISTORY").unwrap();
        Self(root)
    }

    fn core(&self) -> Control {
        let store = Store::open(&self.join("state/sessions/s1/team.db")).unwrap();
        store.create_session("s1", self.join("project").to_str().unwrap(), "approved_scope").unwrap();
        let spec: TeamSpec = serde_json::from_value(json!({
            "leader_id":"leader",
            "agents":[
                {"id":"leader","name":"L","role":"leader","runtime_kind":"deepagents","model_profile":"m"},
                {"id":"b","name":"B","role":"worker","runtime_kind":"deepagents","model_profile":"m"}
            ],
            "channels":[{"source":"leader","targets":["b"],"mode":"task"}],
            "shared_spaces":[{"id":"results","readers":["leader","b"],"writers":["b"]}]
        }))
        .unwrap();
        store.save_team_spec("s1", &spec).unwrap();
        for member in &spec.agents {
            store.ensure_agent("s1", &member.id).unwrap();
        }
        Control::new(store, "s1")
    }
}

impl Deref for Fixture {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn action(id: &str, kind: ActionKind, payload: Json, run: Option<&str>) -> TeamAction {
    TeamAction {
        session_id: "s1".into(),
        action_id: id.into(),
        actor_id: "b".into(),
        kind,
        payload,
        run_id: run.map(str::to_string),
    }
}

fn assigned(ctl: &mut Control) -> TurnRun {
    let mut assign = action("assign", ActionKind::AssignTask, json!({"assignee":"b","description":"deliver"}), None);
    assign.actor_id = "leader".into();
    assert!(ctl.submit(&assign).unwrap().ok);
    let run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    ctl.begin_run(&run.run_id).unwrap();
    ctl.store.get_run(&run.run_id).unwrap().unwrap()
}

fn completed() -> TurnOutcome {
    TurnOutcome { status: TurnStatus::Completed, error: None, note: None, reply_text: None }
}

#[test]
fn shared_references_reject_private_handles_paths_and_invalid_types_without_publishing() {
    let fixture = Fixture::new();
    let mut ctl = fixture.core();
    ctl.store.set_codex_thread("s1", "b", "saved-private-codex-thread").unwrap();
    let private = fixture.join("state/sessions/s1/members/b/chat_tree.json");
    std::os::unix::fs::symlink(&private, fixture.join("project/public-looking.json")).unwrap();
    for name in ["trailing.json ", "decorated.json#L1", "decorated.json:12", "source.json"] {
        std::os::unix::fs::symlink(&private, fixture.join("project").join(name)).unwrap();
    }
    std::fs::create_dir_all(fixture.join("state/sessions/s1/members/b/nested")).unwrap();
    std::os::unix::fs::symlink(
        fixture.join("state/sessions/s1/members/b/nested"),
        fixture.join("state/sessions/s1/artifacts/alias"),
    )
    .unwrap();
    for (index, reference) in [
        json!("ctx:b:1"),
        json!("saved-private-codex-thread"),
        json!("saved-private-codex-thread:12:4"),
        json!("codex:saved-private-codex-thread"),
        json!("/tool-output/exec-private.log"),
        json!("/tool-output/../chat_tree.json"),
        json!(private),
        json!(format!("{}:3:2", private.display())),
        json!(format!("file://localhost{}#L1", private.display())),
        json!(format!("file://{}#L1", private.display()).replace("members", "%6dembers")),
        json!("public-looking.json"),
        json!("trailing.json "),
        json!("decorated.json#L1"),
        json!("decorated.json:12"),
        json!("source.json:12:4"),
        json!("../state/sessions/s1/members/b/chat_tree.json"),
        json!("/artifacts/../members/b/chat_tree.json"),
        json!("/artifacts/alias/../chat_tree.json"),
        json!("/artifacts/exec-legacy.log"),
        json!("//artifacts/exec-legacy.log"),
        json!("./artifacts/exec-legacy.log"),
        json!(fixture.join("state/sessions/old/members/b/chat_history.json")),
        json!(fixture.join("state/sessions/archived/old/members/b/turns/old.json")),
        json!(fixture.join("state/sessions/s1/team.db")),
        json!({"path":"ctx:b:1"}),
        json!(42),
        json!("   "),
        json!("#L1"),
        json!("file://other-host/tmp/result.md"),
        json!("file:12"),
        json!("file:///tmp/%broken"),
        json!("file:///tmp/invalid%00name"),
    ]
    .into_iter()
    .enumerate()
    {
        let input = action(
            &format!("private-{index}"),
            ActionKind::PublishShared,
            json!({"space_id":"results","content":"nonempty content cannot bypass reference validation","ref":reference}),
            None,
        );
        let receipt = ctl.submit(&input).unwrap();
        assert!(!receipt.ok, "private or malformed reference accepted: {reference}");
        assert_eq!(ctl.submit(&input).unwrap().error, receipt.error, "refusal must replay idempotently");
    }
    assert!(ctl.store.shared_entries("s1", &["results".into()], 0, 100).unwrap().is_empty());
    assert!(ctl.store.events("s1", 0, 100).unwrap().is_empty());
    assert!(ctl.store.pending_deliveries("s1", "leader").unwrap().is_empty());

    // Ordinary output files, URLs and explicitly shared artifacts remain valid.
    for (index, reference) in [
        "report.md".to_string(),
        "artifacts/report.md".into(),
        "/artifacts/sub/../report.md".into(),
        "https://example.test/members/b/chat_tree.json#L1".into(),
        "s3://deliverables/results.json".into(),
        "schema.json#definitions/output".into(),
        "src/main.rs:12:4".into(),
        format!("file://{}/report%20name.md", fixture.join("project").display()),
        fixture.join("state/sessions/s1/members/b/work/report.md").to_string_lossy().into_owned(),
    ]
    .into_iter()
    .enumerate()
    {
        let receipt = ctl
            .submit(&action(
                &format!("valid-{index}"),
                ActionKind::PublishShared,
                json!({"space_id":"results","ref":reference}),
                None,
            ))
            .unwrap();
        assert!(receipt.ok, "{reference}: {:?}", receipt.error);
    }
    assert_eq!(ctl.store.shared_entries("s1", &["results".into()], 0, 100).unwrap().len(), 9);
}

#[test]
fn task_result_references_validate_the_whole_array_and_keep_the_last_valid_request() {
    let fixture = Fixture::new();
    let mut ctl = fixture.core();
    let run = assigned(&mut ctl);
    let task = run.task_id.as_deref().unwrap();
    ctl.store.set_run_external_turn(&run.run_id, "saved-private-external-turn").unwrap();
    let good = action(
        "good",
        ActionKind::CompleteTask,
        json!({"task_id":task,"result_refs":["/artifacts/report.md"],"summary":"valid result"}),
        Some(&run.run_id),
    );
    assert!(ctl.submit(&good).unwrap().ok);
    let saved = ctl.store.completion_request(&run.run_id).unwrap();
    for (index, refs) in [
        json!(["report.md", "ctx:b:1"]),
        json!(["saved-private-external-turn"]),
        json!(["report.md", 7]),
        json!(["report.md", {"path":"private"}]),
        json!("report.md"),
        json!([""]),
    ]
    .into_iter()
    .enumerate()
    {
        let refused = ctl
            .submit(&action(
                &format!("bad-{index}"),
                ActionKind::CompleteTask,
                json!({"task_id":task,"result_refs":refs,"summary":"must not overwrite"}),
                Some(&run.run_id),
            ))
            .unwrap();
        assert!(!refused.ok, "invalid result_refs accepted: {refs}");
        assert_eq!(ctl.store.completion_request(&run.run_id).unwrap(), saved);
    }
    ctl.finalize_run(&run.run_id, &completed(), &run.input_delivery_ids).unwrap();
    let task = ctl.store.get_task(task).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Succeeded);
    assert_eq!(task.result_refs, vec!["/artifacts/report.md"]);
}

#[test]
fn pending_private_or_retargeted_references_block_completion_without_publishing() {
    for legacy in [true, false] {
        let fixture = Fixture::new();
        let mut ctl = fixture.core();
        let run = assigned(&mut ctl);
        let task = run.task_id.as_deref().unwrap();
        let reference = if legacy { "ctx:b:1" } else { "report.json" };
        if legacy {
            // A request persisted by an older release must not bypass the new
            // check just because finalization has no fresh model action.
            ctl.store.record_completion_request(&run.run_id, task, &[reference.into()], "legacy").unwrap();
        } else {
            std::fs::write(fixture.join("project/report.json"), "ordinary report").unwrap();
            let receipt = ctl
                .submit(&action(
                    "complete",
                    ActionKind::CompleteTask,
                    json!({"task_id":task,"result_refs":[reference],"summary":"accepted before replacement"}),
                    Some(&run.run_id),
                ))
                .unwrap();
            assert!(receipt.ok);
            std::fs::remove_file(fixture.join("project/report.json")).unwrap();
            std::os::unix::fs::symlink(
                fixture.join("state/sessions/s1/members/b/chat_tree.json"),
                fixture.join("project/report.json"),
            )
            .unwrap();
        }
        ctl.finalize_run(&run.run_id, &completed(), &run.input_delivery_ids).unwrap();
        ctl.finalize_run(&run.run_id, &completed(), &run.input_delivery_ids).unwrap();
        let task = ctl.store.get_task(task).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Blocked, "invalid result was marked successful");
        assert!(task.result_refs.is_empty());
        assert_eq!(ctl.store.get_run(&run.run_id).unwrap().unwrap().status, TurnStatus::Completed);
        let events = ctl.store.events("s1", 0, 100).unwrap();
        assert!(!events.iter().any(|event| event["kind"] == "task_completed"));
        assert_eq!(events.iter().filter(|event| event["kind"] == "task_blocked").count(), 1);
        assert!(events.iter().any(|event| {
            event["kind"] == "run_progress"
                && event["payload"]["text"].as_str().is_some_and(|text| text.contains("完成申请未通过输出引用检查"))
        }));
        assert!(!serde_json::to_string(&events).unwrap().contains("ctx:b:1"));
        assert!(ctl.store.completion_request(&run.run_id).unwrap().is_some(), "keep the original audit evidence");
    }
}

#[test]
fn reference_validation_storage_failure_does_not_settle_or_acknowledge_the_run() {
    let fixture = Fixture::new();
    let mut ctl = fixture.core();
    let run = assigned(&mut ctl);
    ctl.store
        .record_completion_request(&run.run_id, run.task_id.as_deref().unwrap(), &["report.md".into()], "ready")
        .unwrap();
    ctl.store.conn.execute("UPDATE completion_requests SET result_refs='broken JSON'", []).unwrap();
    let result = ctl.finalize_run(&run.run_id, &completed(), &run.input_delivery_ids);
    assert!(result.is_err(), "corrupt stored references were silently treated as an empty array");
    assert_eq!(ctl.store.get_run(&run.run_id).unwrap().unwrap().status, TurnStatus::Running);
    assert_eq!(ctl.store.get_task(run.task_id.as_deref().unwrap()).unwrap().unwrap().status, TaskStatus::Running);
    assert!(!ctl.store.pending_deliveries("s1", "b").unwrap().is_empty());
}

#[test]
fn rejected_reference_settlement_rolls_back_its_status_events_and_deliveries_together() {
    let fixture = Fixture::new();
    let mut ctl = fixture.core();
    let run = assigned(&mut ctl);
    ctl.store
        .record_completion_request(&run.run_id, run.task_id.as_deref().unwrap(), &["ctx:b:1".into()], "legacy")
        .unwrap();
    let before = ctl.store.events("s1", 0, 100).unwrap();
    ctl.store
        .conn
        .execute_batch(
            "CREATE TRIGGER reject_blocked_event BEFORE INSERT ON events
         WHEN NEW.kind='task_blocked' BEGIN SELECT RAISE(ABORT,'blocked event unavailable'); END;",
        )
        .unwrap();
    let error = ctl.finalize_run(&run.run_id, &completed(), &run.input_delivery_ids).unwrap_err();
    assert!(error.contains("blocked event unavailable"), "{error}");
    assert_eq!(ctl.store.get_run(&run.run_id).unwrap().unwrap().status, TurnStatus::Running);
    assert_eq!(ctl.store.get_task(run.task_id.as_deref().unwrap()).unwrap().unwrap().status, TaskStatus::Running);
    assert!(!ctl.store.pending_deliveries("s1", "b").unwrap().is_empty());
    assert_eq!(ctl.store.events("s1", 0, 100).unwrap(), before);
    ctl.store.conn.execute_batch("DROP TRIGGER reject_blocked_event;").unwrap();
    ctl.finalize_run(&run.run_id, &completed(), &run.input_delivery_ids).unwrap();
    assert_eq!(ctl.store.get_task(run.task_id.as_deref().unwrap()).unwrap().unwrap().status, TaskStatus::Blocked);
}
