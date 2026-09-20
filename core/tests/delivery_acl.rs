//! Pending deliveries must satisfy both their original and current grants.

use serde_json::{json, Value as Json};
use teamagents_core::control::{Control, EventDraft};
use teamagents_core::models::*;
use teamagents_core::storage::Store;
use teamagents_core::views;

fn observer(scope: &str) -> Json {
    json!({
        "agent_id": "watch", "subjects": ["worker"], "event_types": ["task_completed"],
        "payload_scope": scope, "wake_policy": "on_event"
    })
}

fn harness(scope: &str) -> Control {
    let spec: TeamSpec = serde_json::from_value(json!({
        "leader_id": "leader",
        "agents": (["leader", "worker", "watch"].map(|id| json!({
            "id": id, "name": id, "role": if id == "leader" { "leader" } else { "worker" },
            "runtime_kind": "deepagents", "model_profile": "m"
        }))),
        "channels": [{"source": "leader", "targets": ["worker"], "mode": "message"}],
        "observers": [observer(scope)]
    }))
    .unwrap();
    let store = Store::open_memory().unwrap();
    store.create_session("acl", "/tmp", "approved_scope").unwrap();
    store.save_team_spec("acl", &spec).unwrap();
    for agent in &spec.agents {
        store.ensure_agent("acl", &agent.id).unwrap();
    }
    Control::new(store, "acl")
}

fn task_event(ctl: &mut Control, id: &str) {
    let mut event = EventDraft::new(
        EventKind::TaskCompleted,
        json!({
            "task_id": id, "assignee": "worker", "requester": "leader", "status": "SUCCEEDED",
            "summary": "PRIVATE SUMMARY", "result_refs": ["PRIVATE-REF"]
        }),
    );
    event.task_id = Some(id.into());
    ctl.emit(vec![event], "worker").unwrap();
}

fn view(ctl: &Control, agent: &str) -> Json {
    let spec = ctl.store.load_team_spec("acl", None).unwrap();
    views::build_agent_view(&ctl.store, &spec, "acl", agent).unwrap()
}

fn park(ctl: &Control, agent: &str) {
    for run in ctl.store.runs_for_session("acl", &[TurnStatus::Queued, TurnStatus::Running]).unwrap() {
        if run.agent_id == agent {
            ctl.store.set_run_status(&run.run_id, TurnStatus::WaitingApproval).unwrap();
        }
    }
}

fn patch(ctl: &mut Control, operations: Json) {
    let action = TeamAction {
        action_id: new_id("patch"),
        session_id: "acl".into(),
        actor_id: "leader".into(),
        run_id: None,
        kind: ActionKind::ApplyTopologyPatch,
        payload: json!({"base_revision": ctl.store.current_revision("acl").unwrap(), "operations": operations}),
    };
    let receipt = ctl.submit(&action).unwrap();
    assert!(receipt.ok, "{:?}", receipt.error);
    assert_eq!(receipt.result["status"], json!("APPLIED"));
}

#[test]
fn queued_observer_deliveries_follow_revocation_and_never_regain_old_payload() {
    for change in ["remove", "scope", "subjects", "events"] {
        let mut ctl = harness("result");
        task_event(&mut ctl, "old-task");
        let before = view(&ctl, "watch");
        assert!(before.to_string().contains("PRIVATE-REF"));
        let delivery = before["delivery_ids"][0].as_i64().unwrap();
        park(&ctl, "watch");
        let mut changed = observer("result");
        match change {
            "remove" => changed = json!({"agent_id": "watch"}),
            "scope" => changed["payload_scope"] = json!("status"),
            "subjects" => changed["subjects"] = json!(["watch"]),
            "events" => changed["event_types"] = json!(["task_failed"]),
            _ => unreachable!(),
        }
        patch(&mut ctl, json!([{"op": "set_observer", "observer": changed, "remove": change == "remove"}]));
        let after = view(&ctl, "watch");
        if change == "scope" {
            assert_eq!(
                after["inbox_delta"][0]["payload"],
                json!({
                    "task_id": "old-task", "assignee": "worker", "requester": "leader", "status": "SUCCEEDED"
                })
            );
        } else {
            assert_eq!(after["inbox_delta"], json!([]), "{change}: {after}");
            assert_eq!(after["delivery_ids"], json!([]));
            let (status, reason): (String, String) = ctl
                .store
                .conn
                .query_row("SELECT status, payload_override FROM deliveries WHERE delivery_id=?1", [delivery], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .unwrap();
            assert_eq!(status, "dropped");
            assert!(serde_json::from_str::<Json>(&reason).unwrap()["dropped_reason"].is_string());
            ctl.store.ack_delivery_by_id(delivery).unwrap();
            assert_eq!(ctl.store.applied_batch("acl", "watch").unwrap(), 0, "revocation is not consumption");
        }
        patch(&mut ctl, json!([{"op": "set_observer", "observer": observer("result")}]));
        let restored = view(&ctl, "watch");
        assert!(!restored.to_string().contains("PRIVATE"), "grant must not widen an old delivery: {restored}");
        assert!(
            json!(ctl.store.events("acl", 0, 100).unwrap()).to_string().contains("PRIVATE-REF"),
            "the audit event is immutable"
        );
    }
}

#[test]
fn buffered_mid_turn_push_is_rechecked_before_drain() {
    let mut ctl = harness("result");
    task_event(&mut ctl, "first");
    let run = ctl
        .store
        .runs_for_session("acl", &[TurnStatus::Queued])
        .unwrap()
        .into_iter()
        .find(|run| run.agent_id == "watch")
        .unwrap();
    ctl.begin_run(&run.run_id).unwrap();
    task_event(&mut ctl, "buffered");
    assert!(!ctl.mid_turn_pushes.is_empty());
    let mut spec = ctl.store.load_team_spec("acl", None).unwrap();
    spec.observers[0].payload_scope = "status".into();
    ctl.store.save_team_spec("acl", &spec).unwrap();
    let pushes = ctl.drain_mid_turn_pushes().unwrap();
    let body = serde_json::to_value(&pushes).unwrap().to_string();
    assert!(!body.contains("PRIVATE"), "buffered old payload crossed the delivery boundary: {body}");
    assert!(body.contains("buffered"));
}

#[test]
fn failed_mid_turn_batch_keeps_routing_and_rechecks_permissions_on_retry() {
    let mut ctl = harness("result");
    task_event(&mut ctl, "first");
    let watch = ctl
        .store
        .runs_for_session("acl", &[TurnStatus::Queued])
        .unwrap()
        .into_iter()
        .find(|run| run.agent_id == "watch")
        .unwrap();
    ctl.begin_run(&watch.run_id).unwrap();
    let message = |id: &str| TeamAction {
        action_id: id.into(),
        session_id: "acl".into(),
        actor_id: "leader".into(),
        run_id: None,
        kind: ActionKind::SendMessage,
        payload: json!({"target":"worker","text":format!("PRIVATE-{id}")}),
    };
    assert!(ctl.submit(&message("start-worker")).unwrap().ok);
    let worker = ctl
        .store
        .runs_for_session("acl", &[TurnStatus::Queued])
        .unwrap()
        .into_iter()
        .find(|run| run.agent_id == "worker")
        .unwrap();
    ctl.begin_run(&worker.run_id).unwrap();
    task_event(&mut ctl, "buffered");
    assert!(ctl.submit(&message("buffered-worker")).unwrap().ok);
    assert_eq!(ctl.mid_turn_pushes.len(), 2);
    let buffered = ctl.mid_turn_pushes.clone();
    let before = ctl.store.pending_deliveries("acl", "watch").unwrap();
    let mut spec = ctl.store.load_team_spec("acl", None).unwrap();
    spec.observers[0].payload_scope = "status".into();
    ctl.store.save_team_spec("acl", &spec).unwrap();
    let input: String = ctl
        .store
        .conn
        .query_row("SELECT input_delivery_ids FROM turn_runs WHERE run_id=?1", [&worker.run_id], |row| row.get(0))
        .unwrap();
    ctl.store.conn.execute("UPDATE turn_runs SET input_delivery_ids='[' WHERE run_id=?1", [&worker.run_id]).unwrap();
    let error = ctl.drain_mid_turn_pushes().unwrap_err();
    assert!(error.contains("input_delivery_ids"), "{error}");
    assert_eq!(ctl.mid_turn_pushes, buffered, "failed batch must preserve all routing and items");
    assert_eq!(
        ctl.store.pending_deliveries("acl", "watch").unwrap(),
        before,
        "a later routing failure must roll back the earlier scope projection"
    );
    ctl.store
        .conn
        .execute("UPDATE turn_runs SET input_delivery_ids=?1 WHERE run_id=?2", [&input, &worker.run_id])
        .unwrap();
    // Revoke a buffered recipient before retry. The original payload is not a
    // grant, even when a previous attempt already resolved its routing.
    spec.channels.clear();
    ctl.store.save_team_spec("acl", &spec).unwrap();
    let pushes = ctl.drain_mid_turn_pushes().unwrap();
    assert_eq!(pushes.len(), 1);
    assert_eq!(pushes[0].run_id, watch.run_id);
    assert_eq!(pushes[0].agent_id, "watch");
    let body = serde_json::to_value(&pushes).unwrap().to_string();
    assert!(body.contains("buffered") && !body.contains("PRIVATE"), "{body}");
    assert!(ctl.store.pending_deliveries("acl", "worker").unwrap().is_empty());
    assert!(ctl.drain_mid_turn_pushes().unwrap().is_empty(), "a successful batch drains exactly once");
    assert_eq!(ctl.store.applied_batch("acl", "worker").unwrap(), 0, "revocation is not consumption");
}

#[test]
fn queued_direct_message_is_dropped_when_channel_is_revoked() {
    let mut ctl = harness("status");
    let sent = ctl
        .submit(&TeamAction {
            action_id: "message".into(),
            session_id: "acl".into(),
            actor_id: "leader".into(),
            run_id: None,
            kind: ActionKind::SendMessage,
            payload: json!({"target": "worker", "text": "PRIVATE-MESSAGE"}),
        })
        .unwrap();
    assert!(sent.ok);
    park(&ctl, "worker");
    patch(&mut ctl, json!([{"op": "remove_channel", "source": "leader", "targets": ["worker"]}]));
    assert_eq!(view(&ctl, "worker")["inbox_delta"], json!([]));
    assert!(ctl.store.pending_deliveries("acl", "worker").unwrap().is_empty());
}

#[test]
fn invalid_observer_override_never_falls_back_to_the_raw_payload() {
    let mut ctl = harness("status");
    task_event(&mut ctl, "secret-task");
    ctl.store.conn.execute("UPDATE deliveries SET payload_override='{' WHERE agent_id='watch'", []).unwrap();
    let before = ctl.store.pending_deliveries("acl", "watch").unwrap();
    let error = ctl.agent_view("watch").unwrap_err();
    assert!(error.contains("payload_override"), "{error}");
    assert!(!error.contains("PRIVATE"));
    assert_eq!(ctl.store.pending_deliveries("acl", "watch").unwrap(), before, "keep the unreadable input for repair");
}

#[test]
fn original_audience_and_payload_remain_limits_after_a_new_grant() {
    let mut ctl = harness("status");
    task_event(&mut ctl, "status-only");
    park(&ctl, "watch");
    patch(&mut ctl, json!([{"op": "set_observer", "observer": observer("result")}]));
    assert!(!view(&ctl, "watch").to_string().contains("PRIVATE"));

    patch(&mut ctl, json!([{"op": "set_observer", "observer": {"agent_id": "watch"}, "remove": true}]));
    task_event(&mut ctl, "no-observer");
    let event = ctl.store.events("acl", 0, 100).unwrap().into_iter().find(|e| e["task_id"] == "no-observer").unwrap();
    patch(&mut ctl, json!([{"op": "set_observer", "observer": observer("result")}]));
    // A legacy explicit push or corrupted row cannot enlarge the old audience.
    let id = ctl.store.create_delivery("acl", "watch", event["event_id"].as_str().unwrap(), 2, None).unwrap();
    assert!(ctl.delivery_items("watch", &[id]).unwrap().is_empty());
}

#[test]
fn shared_delivery_loses_read_access_but_task_receipts_keep_their_return_path() {
    let mut ctl = harness("status");
    let mut spec = ctl.store.load_team_spec("acl", None).unwrap();
    spec.shared_spaces = serde_json::from_value(json!([{
        "id": "shared", "readers": ["watch"], "writers": ["worker"]
    }]))
    .unwrap();
    ctl.store.save_team_spec("acl", &spec).unwrap();
    let mut published = EventDraft::new(
        EventKind::SharedPublished,
        json!({"space_id": "shared", "author": "worker", "summary": "PRIVATE-SHARED"}),
    );
    published.push = Some(vec!["watch".into()]);
    ctl.emit(vec![published], "worker").unwrap();
    assert!(view(&ctl, "watch").to_string().contains("PRIVATE-SHARED"));
    park(&ctl, "watch");
    patch(&mut ctl, json!([{"op": "set_space_acl", "space_id": "shared", "readers": []}]));
    assert_eq!(view(&ctl, "watch")["inbox_delta"], json!([]));

    // Task results do not need a generic reverse message channel.
    task_event(&mut ctl, "receipt");
    assert!(view(&ctl, "leader").to_string().contains("PRIVATE-REF"));
    let help = ctl
        .submit(&TeamAction {
            action_id: "help".into(),
            session_id: "acl".into(),
            actor_id: "worker".into(),
            run_id: None,
            kind: ActionKind::RequestHelp,
            payload: json!({"message": "help without a channel"}),
        })
        .unwrap();
    assert!(help.ok);
    assert!(view(&ctl, "leader").to_string().contains("help without a channel"));
}

#[test]
fn delivery_rejection_failure_rolls_back_the_permission_change() {
    let mut ctl = harness("result");
    task_event(&mut ctl, "old-task");
    park(&ctl, "watch");
    let revision = ctl.store.current_revision("acl").unwrap();
    ctl.store
        .conn
        .execute_batch(
            "CREATE TRIGGER deny_delivery_drop BEFORE UPDATE ON deliveries WHEN NEW.status='dropped'
         BEGIN SELECT RAISE(ABORT, 'audit unavailable'); END;",
        )
        .unwrap();
    let receipt = ctl
        .submit(&TeamAction {
            action_id: "failed-patch".into(),
            session_id: "acl".into(),
            actor_id: "leader".into(),
            run_id: None,
            kind: ActionKind::ApplyTopologyPatch,
            payload: json!({"base_revision": revision, "operations": [
                {"op": "set_observer", "observer": {"agent_id": "watch"}, "remove": true}
            ]}),
        })
        .unwrap();
    assert!(!receipt.ok);
    assert!(receipt.error.unwrap().contains("audit unavailable"));
    assert_eq!(ctl.store.current_revision("acl").unwrap(), revision);
    assert!(view(&ctl, "watch").to_string().contains("PRIVATE-REF"), "the grant did not commit");
}

#[test]
fn external_acceptance_ledger_does_not_consume_offers_or_accept_foreign_ids() {
    let mut ctl = harness("result");
    task_event(&mut ctl, "input");
    let run = ctl
        .store
        .runs_for_session("acl", &[TurnStatus::Queued])
        .unwrap()
        .into_iter()
        .find(|run| run.agent_id == "watch")
        .unwrap();
    let ids: Vec<i64> = serde_json::from_value(view(&ctl, "watch")["delivery_ids"].clone()).unwrap();
    assert!(ctl.confirmed_delivery_ids(&run.run_id).unwrap().is_empty());
    let leader_id = view(&ctl, "leader")["delivery_ids"][0].as_i64().unwrap();
    assert!(ctl.confirm_delivery_ids(&run.run_id, &[ids[0], leader_id]).is_err());
    assert!(ctl.confirmed_delivery_ids(&run.run_id).unwrap().is_empty(), "confirmation is atomic");
    ctl.confirm_delivery_ids(&run.run_id, &ids).unwrap();
    ctl.confirm_delivery_ids(&run.run_id, &ids).unwrap();
    assert_eq!(ctl.confirmed_delivery_ids(&run.run_id).unwrap(), ids);
    assert_eq!(ctl.store.applied_batch("acl", "watch").unwrap(), 0);
    assert_eq!(ctl.store.pending_deliveries("acl", "watch").unwrap().len(), 1);
    ctl.finalize_run(
        &run.run_id,
        &teamagents_core::control::TurnOutcome {
            status: TurnStatus::Completed,
            error: None,
            note: None,
            reply_text: None,
        },
        &[],
    )
    .unwrap();
    ctl.confirm_delivery_ids(&run.run_id, &ids).unwrap();
    assert!(
        ctl.store.pending_deliveries("acl", "watch").unwrap().is_empty(),
        "a late external acknowledgement must not leave a terminal turn's input replayable"
    );
    assert_eq!(ctl.store.applied_batch("acl", "watch").unwrap(), 1);
}

#[test]
fn unknown_input_is_audited_without_acknowledgement_or_automatic_replay() {
    use teamagents_core::control::TurnOutcome;
    let mut ctl = harness("result");
    task_event(&mut ctl, "accepted");
    let run = ctl
        .store
        .runs_for_session("acl", &[TurnStatus::Queued])
        .unwrap()
        .into_iter()
        .find(|r| r.agent_id == "watch")
        .unwrap();
    ctl.begin_run(&run.run_id).unwrap();
    let accepted = run.input_delivery_ids.clone();
    task_event(&mut ctl, "uncertain");
    let all = ctl.store.get_run(&run.run_id).unwrap().unwrap().input_delivery_ids;
    assert_eq!(all.len(), 2);
    let unknown = all.iter().find(|id| !accepted.contains(id)).copied().unwrap();
    ctl.confirm_delivery_ids(&run.run_id, &accepted).unwrap();
    let outcome = TurnOutcome {
        status: TurnStatus::OutcomeUnknown,
        error: Some("external reply was lost".into()),
        note: None,
        reply_text: None,
    };
    ctl.store
        .conn
        .execute_batch(
            "CREATE TRIGGER deny_unknown_drop BEFORE UPDATE ON deliveries WHEN NEW.status='dropped'
         BEGIN SELECT RAISE(ABORT, 'audit unavailable'); END;",
        )
        .unwrap();
    assert!(ctl.finalize_run(&run.run_id, &outcome, &accepted).unwrap_err().contains("audit unavailable"));
    assert_eq!(ctl.store.get_run(&run.run_id).unwrap().unwrap().status, TurnStatus::Running);
    assert_eq!(ctl.store.applied_batch("acl", "watch").unwrap(), 0, "failed audit rolls back acknowledgements");
    ctl.store.conn.execute_batch("DROP TRIGGER deny_unknown_drop").unwrap();
    ctl.finalize_run(&run.run_id, &outcome, &accepted).unwrap();
    let (status, payload): (String, String) = ctl
        .store
        .conn
        .query_row("SELECT status,payload_override FROM deliveries WHERE delivery_id=?1", [unknown], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(status, "dropped");
    assert_eq!(serde_json::from_str::<Json>(&payload).unwrap()["run_id"], run.run_id);
    assert_eq!(ctl.store.applied_batch("acl", "watch").unwrap(), 1);
    assert_eq!(ctl.confirmed_delivery_ids(&run.run_id).unwrap(), accepted);
    assert!(ctl.delivery_items("watch", &all).unwrap().is_empty(), "stale buffers also cannot replay uncertain input");
    ctl.schedule().unwrap();
    assert!(!ctl.store.runs_for_session("acl", &[TurnStatus::Queued]).unwrap().iter().any(|r| r.agent_id == "watch"));
    // Explicitly accepting the uncertain outcome must not resurrect its input.
    let receipt = ctl
        .submit(&TeamAction {
            action_id: "ack-unknown".into(),
            session_id: "acl".into(),
            actor_id: "leader".into(),
            run_id: None,
            kind: ActionKind::CancelRun,
            payload: json!({"run_id":run.run_id}),
        })
        .unwrap();
    assert!(receipt.ok, "{receipt:?}");
    assert!(ctl.store.pending_deliveries("acl", "watch").unwrap().is_empty());
    task_event(&mut ctl, "new-work");
    assert_eq!(ctl.store.pending_deliveries("acl", "watch").unwrap().len(), 1, "new explicit work remains runnable");
}

#[test]
fn scope_only_patch_waits_for_the_receiving_observers_execution_boundary() {
    let mut ctl = harness("result");
    task_event(&mut ctl, "first");
    let run = ctl
        .store
        .runs_for_session("acl", &[TurnStatus::Queued])
        .unwrap()
        .into_iter()
        .find(|run| run.agent_id == "watch")
        .unwrap();
    ctl.begin_run(&run.run_id).unwrap();
    task_event(&mut ctl, "buffered");
    let receipt = ctl
        .submit(&TeamAction {
            action_id: "scope-boundary".into(),
            session_id: "acl".into(),
            actor_id: "leader".into(),
            run_id: None,
            kind: ActionKind::ApplyTopologyPatch,
            payload: json!({"base_revision": ctl.store.current_revision("acl").unwrap(), "operations": [
                {"op": "set_observer", "observer": observer("status")}
            ]}),
        })
        .unwrap();
    assert!(receipt.ok, "{:?}", receipt.error);
    assert_eq!(receipt.result["status"], json!("WAITING_BOUNDARY"));
    assert_eq!(receipt.result["affected_agents"], json!(["watch"]), "the observed worker's rights did not change");
    ctl.finalize_run(
        &run.run_id,
        &teamagents_core::control::TurnOutcome {
            status: TurnStatus::Completed,
            error: None,
            note: None,
            reply_text: None,
        },
        &[],
    )
    .unwrap();
    assert_eq!(
        ctl.store.get_patch(receipt.result["patch_id"].as_str().unwrap()).unwrap().unwrap().status,
        PatchStatus::Applied
    );
    let drained = ctl.drain_mid_turn_pushes().unwrap();
    assert!(drained.is_empty(), "ended turns must not receive a buffered push");
    assert!(!view(&ctl, "watch").to_string().contains("PRIVATE"));
}
