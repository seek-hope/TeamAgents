#[test]
fn autonomy_retention_preserves_pending_acceptance_until_work_closes() {
    let (mut ctl, path) = completed_autonomous_control("work-retention");
    ctl.submit(cmd("park", "set_lifecycle", json!({"instance_id":"i1","lifecycle":"PARKED"})), Identity::User).unwrap();
    ctl.submit(
        cmd(
            "checked-input",
            "submit_input",
            json!({"instance_id":"i1","envelope_id":"checked-input",
        "text":"Build the next artifact", "checks":[{"id":"artifact","command":"test -f artifact.txt"}]}),
        ),
        Identity::User,
    )
    .unwrap();
    ctl.submit(
        cmd(
            "followup",
            "submit_input",
            json!({"instance_id":"i1","envelope_id":"followup",
        "text":"Preserve the public API"}),
        ),
        Identity::User,
    )
    .unwrap();
    assert_eq!(work_state(ctl.connection(), "i1").unwrap()["state"], "STOPPED");
    let applied: i64 = ctl
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM envelopes WHERE id IN ('checked-input','followup') AND state='APPLIED'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(applied, 2, "both parked inputs have been recorded in context");
    ctl.connection()
        .execute("UPDATE context_entries SET created=0 WHERE envelope_id IN ('checked-input','followup')", [])
        .unwrap();
    let sweep = json!({"days":7,"now":crate::models::now(),"evidence":false});
    ctl.submit(cmd("sweep-pending", "prune_history", sweep.clone()), Identity::User).unwrap();
    let retained: i64 = ctl
        .connection()
        .query_row("SELECT COUNT(*) FROM envelopes WHERE id IN ('checked-input','followup')", [], |r| r.get(0))
        .unwrap();
    assert_eq!(retained, 2, "pending work still owns its input payloads and acceptance checks");
    ctl.submit(cmd("resume", "set_lifecycle", json!({"instance_id":"i1","lifecycle":"ACTIVE"})), Identity::User)
        .unwrap();
    let state = work_state(ctl.connection(), "i1").unwrap();
    let goal = state["goal_id"].as_str().expect("the saved acceptance must activate work");
    assert_ne!(goal, "g1");
    let limits: String =
        ctl.connection().query_row("SELECT limits_json FROM goals WHERE id=?1", [goal], |r| r.get(0)).unwrap();
    let limits: Json = serde_json::from_str(&limits).unwrap();
    assert_eq!(limits["required_checks"][0]["command"], "test -f artifact.txt");
    assert_eq!(work_context(ctl.connection(), "i1").unwrap()["recent_user_inputs"].as_array().unwrap().len(), 2);
    ctl.submit(cmd("sweep-active", "prune_history", sweep.clone()), Identity::User).unwrap();
    let retained: bool = ctl
        .connection()
        .query_row("SELECT EXISTS(SELECT 1 FROM envelopes WHERE id='checked-input')", [], |r| r.get(0))
        .unwrap();
    assert!(retained, "active work also owns its assigned input");
    ctl.submit(cmd("cancel", "cancel_goal", json!({"goal_id":goal})), Identity::User).unwrap();
    ctl.submit(cmd("sweep-closed", "prune_history", sweep), Identity::User).unwrap();
    let retained: bool = ctl
        .connection()
        .query_row("SELECT EXISTS(SELECT 1 FROM envelopes WHERE id='checked-input')", [], |r| r.get(0))
        .unwrap();
    assert!(!retained, "closing work releases the old delivery to ordinary retention");
    drop(ctl);
    cleanup(&path);
}

#[test]
fn autonomy_admission_keeps_unknown_usage_and_the_original_deadline() {
    let (mut ctl, path) = completed_autonomous_control("admission-unknown");
    ctl.submit(
        cmd(
            "next",
            "submit_input",
            json!({"instance_id":"i1","envelope_id":"next","text":"Implement the next change"}),
        ),
        Identity::User,
    )
    .unwrap();
    let revision = load_instance(ctl.connection(), "i1").unwrap().4;
    ctl.submit(
        cmd("begin-next", "begin_request", json!({"instance_id":"i1","request_id":"r-next","revision":revision})),
        Identity::Instance("i1".into()),
    )
    .unwrap();
    ctl.submit(cmd("lost","record_attempt",json!({"request_id":"r-next","attempt_id":"a-lost","status":"FAILED","error_class":"lost","unknown_usage":true})),Identity::System).unwrap();
    drop(ctl);
    let mut ctl = Control::open(&path, "s1", false).unwrap();
    ctl.submit(cmd("priced","record_attempt",json!({"request_id":"r-next","attempt_id":"a-priced","status":"COMPLETE","usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15},"raw_ref":"raw"})),Identity::System).unwrap();
    let params = json!({"request_id":"r-next","decision_id":"d-next","entry":{"role":"assistant","content":""},"intents":[],"checkpoint":{"action":"start_work","objective":"Implement the next change"}});
    ctl.submit(cmd("admit-next", "import_response", params.clone()), Identity::System).unwrap();
    ctl.submit(cmd("admit-next", "import_response", params), Identity::System).unwrap();
    let goal = budget_goal(ctl.connection(), "i1").unwrap().unwrap();
    let (raw, unknown): (String, i64) = ctl
        .connection()
        .query_row("SELECT known_usage_json,unknown_usage FROM goals WHERE id=?1", [&goal], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(serde_json::from_str::<crate::kernel::Usage>(&raw).unwrap().total, 15);
    assert_eq!(unknown, 1);
    let original: Option<String> = ctl
        .connection()
        .query_row("SELECT goal_id FROM model_requests WHERE request_id='r-next'", [], |r| r.get(0))
        .unwrap();
    assert!(original.is_none());
    drop(ctl);
    cleanup(&path);

    let (mut ctl, path) = completed_autonomous_control("admission-deadline");
    ctl.submit(
        cmd("policy", "configure_work", json!({"leader_id":"i1","limits":{"deadline_minutes":1}})),
        Identity::User,
    )
    .unwrap();
    ctl.submit(
        cmd(
            "input",
            "submit_input",
            json!({"instance_id":"i1","envelope_id":"pending-deadline","text":"Plan another change"}),
        ),
        Identity::User,
    )
    .unwrap();
    ctl.connection()
        .execute(
            "UPDATE context_entries SET created=?1 WHERE envelope_id='pending-deadline'",
            [crate::models::now() - 120.0],
        )
        .unwrap();
    ctl.submit(
        cmd("later-policy", "configure_work", json!({"leader_id":"i1","limits":{"deadline_minutes":999}})),
        Identity::User,
    )
    .unwrap();
    drop(ctl);
    let mut ctl = Control::open(&path, "s1", false).unwrap();
    let revision = load_instance(ctl.connection(), "i1").unwrap().4;
    let result = ctl
        .submit(
            cmd("too-late", "begin_request", json!({"instance_id":"i1","request_id":"r-too-late","revision":revision})),
            Identity::Instance("i1".into()),
        )
        .unwrap();
    assert_eq!(result["budget_refused"], true);
    assert!(result["reason"].as_str().unwrap().contains("deadline"));
    assert_eq!(load_instance(ctl.connection(), "i1").unwrap().2, "PARKED");
    assert!(budget_goal(ctl.connection(), "i1").unwrap().is_none());
    drop(ctl);
    cleanup(&path);
}

#[test]
fn autonomy_old_response_cannot_replace_a_new_assignments_checkpoint_or_recovery() {
    for checkpoint in [false, true] {
        let (mut ctl, path) = autonomous_control(&format!("work-late-response-{checkpoint}"));
        prepare_work_member(&mut ctl);
        let revision = load_instance(ctl.connection(), "i2").unwrap().4;
        let request = begin_and_complete(&mut ctl, "old", "i2", revision);
        ctl.submit(cmd("cancel-old", "cancel_task", json!({"task_id":"task-1"})), Identity::User).unwrap();
        ctl.submit(
            cmd(
                "delegate-new",
                "delegate_task",
                json!({"task_id":"task-2","assignee":"i2","description":"New assignment"}),
            ),
            Identity::Instance("i1".into()),
        )
        .unwrap();
        ctl.submit(cmd("start-new", "start_task", json!({"task_id":"task-2"})), Identity::Instance("i2".into()))
            .unwrap();
        let before = work_state(ctl.connection(), "i2").unwrap();
        let mut params = json!({"request_id":request,"decision_id":"d-old","intents":[],"entry":{"role":"assistant","content":"Old progress"}});
        if checkpoint {
            let claim = json!({"action":"ask_user","question":"An obsolete question","remaining":["Old requirement"]});
            params["checkpoint"] = claim.clone();
            params["entry"] = json!({"role":"assistant","tool_calls":[{"id":"q-old","function":{"name":"work_checkpoint","arguments":claim.to_string()}}]});
        }
        ctl.submit(cmd("late-response", "import_response", params), Identity::System).unwrap();
        assert_eq!(work_state(ctl.connection(), "i2").unwrap(), before);
        assert_eq!(before["task_id"], "task-2");
        assert_eq!(before["state"], "RUNNING");
        drop(ctl);
        cleanup(&path);
    }
}

#[test]
fn autonomy_preparation_is_fenced_against_new_input_and_assignment_changes() {
    let (mut ctl, path) = autonomous_control("work-prepare-race");
    let revision = load_instance(ctl.connection(), "i1").unwrap().4;
    ctl.submit(
        cmd(
            "new-steering",
            "submit_input",
            json!({"instance_id":"i1","envelope_id":"new-steering","text":"Keep compatibility"}),
        ),
        Identity::User,
    )
    .unwrap();
    for method in ["begin_request", "begin_compression"] {
        let error = ctl
            .submit(
                cmd(
                    &format!("stale-input-{method}"),
                    method,
                    json!({"instance_id":"i1","request_id":format!("stale-input-{method}"),"revision":revision}),
                ),
                Identity::System,
            )
            .unwrap_err();
        assert!(error.contains("stale executor"), "{error}");
    }
    prepare_work_member(&mut ctl);
    let prepared = work_context(ctl.connection(), "i2").unwrap();
    ctl.submit(cmd("cancel-prepared", "cancel_task", json!({"task_id":"task-1"})), Identity::User).unwrap();
    ctl.submit(
        cmd(
            "assign-during-prepare",
            "delegate_task",
            json!({"task_id":"task-2","assignee":"i2","description":"A different assignment"}),
        ),
        Identity::Instance("i1".into()),
    )
    .unwrap();
    let revision = load_instance(ctl.connection(), "i2").unwrap().4;
    for method in ["begin_request", "begin_compression"] {
        let error=ctl.submit(cmd(&format!("stale-scope-{method}"),method,json!({"instance_id":"i2","request_id":format!("stale-scope-{method}"),"revision":revision,"prepared_work":prepared})),Identity::System).unwrap_err();
        assert!(error.contains("work changed during request preparation"), "{error}");
    }
    let count: i64 = ctl.connection().query_row("SELECT COUNT(*) FROM model_requests", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 0, "stale preparations cannot start or reserve model calls");
    drop(ctl);
    cleanup(&path);
}

#[test]
fn autonomy_queued_input_preserves_order_and_supersedes_a_later_wait() {
    for waiting in [false, true] {
        let (mut ctl, path) = autonomous_control(if waiting { "input-wait-order" } else { "input-ready-order" });
        prepare_work_member(&mut ctl);
        let revision = load_instance(ctl.connection(), "i1").unwrap().4;
        let request = begin_and_complete(&mut ctl, "input-race", "i1", revision);
        ctl.submit(
            cmd("older", "submit_input", json!({"instance_id":"i1","envelope_id":"older","text":"Use option A"})),
            Identity::User,
        )
        .unwrap();
        let mut response = json!({"request_id":request,"decision_id":"input-race","entry":{"role":"assistant","content":"Progress"},"intents":[]});
        if waiting {
            let wait = json!({"mode":"ALL","conditions":[{"kind":"task","task_id":"task-1"}]});
            response["wait"] = wait.clone();
            response["entry"] = json!({"role":"assistant","content":"","tool_calls":[{"id":"wait-call","type":"function","function":{"name":"wait","arguments":wait.to_string()}}]});
        }
        ctl.submit(cmd("input-race-response", "import_response", response), Identity::System).unwrap();
        let newer = ctl
            .submit(
                cmd(
                    "newer",
                    "submit_input",
                    json!({"instance_id":"i1","envelope_id":"newer","text":"Correction: use option B"}),
                ),
                Identity::User,
            )
            .unwrap();
        assert_eq!(newer["queued"], true);
        ctl.submit(cmd("drain-ordered", "drain_inbox", json!({"instance_id":"i1"})), Identity::Instance("i1".into()))
            .unwrap();
        let mut stmt = ctl
            .connection()
            .prepare("SELECT envelope_id FROM context_entries WHERE envelope_id IN ('older','newer') ORDER BY idx")
            .unwrap();
        let order = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(order, vec!["older", "newer"]);
        assert_eq!(phase_of(&ctl, "i1"), "READY");
        assert_eq!(work_state(ctl.connection(), "i1").unwrap()["state"], "RUNNING");
        if waiting {
            let status: String = ctl
                .connection()
                .query_row("SELECT status FROM waits WHERE id='w-input-race'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(status, "CANCELLED");
        }
        drop(stmt);
        drop(ctl);
        cleanup(&path);
    }
}

#[test]
fn autonomy_fork_preserves_pending_cost_policy_and_user_stop() {
    let (mut source, path) = completed_autonomous_control("work-fork-cost");
    source
        .submit(
            cmd("small-policy", "configure_work", json!({"leader_id":"i1","limits":{"max_total_tokens":20}})),
            Identity::User,
        )
        .unwrap();
    source
        .submit(
            cmd(
                "new-input",
                "submit_input",
                json!({"instance_id":"i1","envelope_id":"new-input","text":"Build the new endpoint"}),
            ),
            Identity::User,
        )
        .unwrap();
    work_turn(
        &mut source,
        "choose-api",
        "i1",
        json!({"checkpoint":{"action":"ask_user","question":"Which API shape is required?","decisions":["Keep the existing authentication"]}}),
    );
    source
        .submit(
            cmd("park-source", "set_lifecycle", json!({"instance_id":"i1","lifecycle":"PARKED","reason":"User pause"})),
            Identity::User,
        )
        .unwrap();
    let fork_path = db_path("work-fork-copy");
    store::fork_database(&path, &fork_path).unwrap();
    let mut copy = Control::open(&fork_path, "s1", false).unwrap();
    copy.submit(cmd("fork", "fork_reset", json!({"keep_instance":"i1"})), Identity::User).unwrap();
    assert_eq!(load_instance(copy.connection(), "i1").unwrap().2, "PARKED");
    assert_eq!(work_state(copy.connection(), "i1").unwrap()["carried_usage"]["total"], 15);
    copy.submit(
        cmd("future-policy", "configure_work", json!({"leader_id":"i1","limits":{"max_total_tokens":500}})),
        Identity::User,
    )
    .unwrap();
    copy.submit(
        cmd("answer", "submit_input", json!({"instance_id":"i1","envelope_id":"answer","text":"Use REST"})),
        Identity::User,
    )
    .unwrap();
    assert_eq!(work_state(copy.connection(), "i1").unwrap()["state"], "STOPPED");
    assert_eq!(work_state(copy.connection(), "i1").unwrap()["limits"]["max_total_tokens"], 20);
    copy.submit(cmd("resume-copy", "set_lifecycle", json!({"instance_id":"i1","lifecycle":"ACTIVE"})), Identity::User)
        .unwrap();
    let revision = load_instance(copy.connection(), "i1").unwrap().4;
    let refused=copy.submit(cmd("too-expensive","begin_request",json!({"instance_id":"i1","revision":revision,"request_id":"too-expensive","request_ref":"fake","est_prompt_tokens":10})),Identity::System).unwrap();
    assert_eq!(refused["budget_refused"], true);
    let requests: i64 = copy.connection().query_row("SELECT COUNT(*) FROM model_requests", [], |r| r.get(0)).unwrap();
    assert_eq!(requests, 0);
    let source_inputs: i64 = source
        .connection()
        .query_row("SELECT COUNT(*) FROM work_inputs WHERE state='PENDING'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(source_inputs, 1, "forking does not change the source admission");
    drop(copy);
    drop(source);
    cleanup(&fork_path);
    cleanup(&path);
}

#[test]
fn autonomy_retired_check_options_are_rejected_before_input_admission() {
    let (mut ctl, path) = completed_autonomous_control("work-retired-check");
    for network in [true, false] {
        let error = ctl
            .submit(
                cmd(
                    &format!("retired-{network}"),
                    "submit_input",
                    json!({"instance_id":"i1","envelope_id":format!("retired-{network}"),"text":"New work",
            "checks":[{"id":"legacy","command":"true","network":network}]}),
                ),
                Identity::User,
            )
            .unwrap_err();
        assert!(error.contains("network is retired"), "{error}");
    }
    let goals: i64 = ctl.connection().query_row("SELECT COUNT(*) FROM goals", [], |r| r.get(0)).unwrap();
    assert_eq!(goals, 1);
    drop(ctl);
    cleanup(&path);
}

#[test]
fn autonomy_queued_steering_is_applied_before_the_existing_goal_settles() {
    let (mut ctl, path) = autonomous_control("work-steering-finish");
    let revision: i64 =
        ctl.connection().query_row("SELECT revision FROM instances WHERE id='i1'", [], |r| r.get(0)).unwrap();
    let request = begin_and_complete(&mut ctl, "finishing", "i1", revision);
    ctl.submit(
        cmd(
            "steering",
            "submit_input",
            json!({"instance_id":"i1","envelope_id":"steering","text":"Also keep the public API compatible."}),
        ),
        Identity::User,
    )
    .unwrap();
    ctl.submit(
        cmd(
            "old-finish",
            "import_response",
            json!({"request_id":request,"decision_id":"old-finish","entry":{"role":"assistant","content":"Done"},
        "intents":[],"completion":{"outcome":"success","summary":"Done before the new input"}}),
        ),
        Identity::System,
    )
    .unwrap();
    let deferred = ctl
        .submit(cmd("settle-too-soon", "complete_goal", json!({"instance_id":"i1","goal_id":"g1"})), Identity::System)
        .unwrap();
    assert_eq!(deferred["completion_deferred"], true);
    assert_eq!(deferred["status"], "ACTIVE");
    ctl.submit(cmd("apply-steering", "drain_inbox", json!({"instance_id":"i1"})), Identity::Instance("i1".into()))
        .unwrap();
    assert_eq!(work_state(ctl.connection(), "i1").unwrap()["goal_id"], "g1");
    work_turn(
        &mut ctl,
        "final-compatible",
        "i1",
        json!({"completion":{"outcome":"success","summary":"Completed with the public API preserved"}}),
    );
    let settled = ctl
        .submit(cmd("settle-current", "complete_goal", json!({"instance_id":"i1","goal_id":"g1"})), Identity::System)
        .unwrap();
    assert_eq!(settled["status"], "SUCCEEDED");
    let goals: i64 = ctl.connection().query_row("SELECT COUNT(*) FROM goals", [], |r| r.get(0)).unwrap();
    assert_eq!(goals, 1);
    drop(ctl);
    cleanup(&path);
}

#[test]
fn autonomy_old_completion_cannot_settle_a_new_assignment() {
    let (mut ctl, path) = autonomous_control("work-stale-finish");
    prepare_work_member(&mut ctl);
    work_turn(
        &mut ctl,
        "old-task-finish",
        "i2",
        json!({"completion":{"outcome":"success","summary":"Old task completed"}}),
    );
    ctl.submit(cmd("cancel-old-task", "cancel_task", json!({"task_id":"task-1"})), Identity::User).unwrap();
    ctl.submit(
        cmd(
            "delegate-next",
            "delegate_task",
            json!({"task_id":"task-2","assignee":"i2","description":"Handle the new assignment"}),
        ),
        Identity::Instance("i1".into()),
    )
    .unwrap();
    ctl.submit(cmd("start-next", "start_task", json!({"task_id":"task-2"})), Identity::Instance("i2".into())).unwrap();
    let refused = ctl
        .submit(
            cmd(
                "stale-settlement",
                "complete_task",
                json!({"task_id":"task-2","status":"SUCCEEDED","summary":"Old task completed"}),
            ),
            Identity::Instance("i2".into()),
        )
        .unwrap();
    assert_eq!(refused["completion_deferred"], true);
    assert_eq!(load_task(ctl.connection(), "s1", "task-2").unwrap().3, "RUNNING");
    assert_eq!(load_task(ctl.connection(), "s1", "task-1").unwrap().3, "CANCELLED");
    drop(ctl);
    cleanup(&path);
}

#[test]
fn autonomy_cancelled_execution_blocks_shared_takeover_until_observed_stopped() {
    let (mut ctl, path) = autonomous_control("work-takeover");
    prepare_work_member(&mut ctl);
    let revision: i64 =
        ctl.connection().query_row("SELECT revision FROM instances WHERE id='i2'", [], |r| r.get(0)).unwrap();
    open_decision(&mut ctl, "old-worker", "i2", revision, 1);
    ctl.submit(cmd("dispatch-old", "dispatch_operation", json!({"operation_id":"d-old-worker:0"})), Identity::System)
        .unwrap();
    ctl.submit(cmd("cancel-old", "cancel_task", json!({"task_id":"task-1"})), Identity::User).unwrap();
    ctl.submit(
        cmd(
            "unknown-old",
            "complete_operation",
            json!({"operation_id":"d-old-worker:0","status":"OUTCOME_UNKNOWN",
        "receipt":{"operation_id":"d-old-worker:0","started":true,"content":"unknown outcome"}}),
        ),
        Identity::System,
    )
    .unwrap();
    let original: String = ctl
        .connection()
        .query_row("SELECT receipt_json FROM operations WHERE operation_id='d-old-worker:0'", [], |r| r.get(0))
        .unwrap();
    let revision: i64 =
        ctl.connection().query_row("SELECT revision FROM instances WHERE id='i1'", [], |r| r.get(0)).unwrap();
    open_decision(&mut ctl, "takeover", "i1", revision, 1);
    let blocked = ctl
        .submit(cmd("try-shared", "dispatch_operation", json!({"operation_id":"d-takeover:0"})), Identity::System)
        .unwrap();
    assert_eq!(blocked["status"], "WORKSPACE_BUSY");
    assert_eq!(blocked["blocker"]["operation_id"], "d-old-worker:0");
    ctl.submit(
        cmd("isolated", "create_instance", json!({"id":"i3","workspace_ref":"/tmp/independent-work"})),
        Identity::User,
    )
    .unwrap();
    ctl.submit(cmd("isolated-goal", "create_goal", json!({"id":"g3","instance_id":"i3"})), Identity::User).unwrap();
    ctl.submit(
        cmd(
            "isolated-input",
            "submit_input",
            json!({"instance_id":"i3","envelope_id":"isolated-input","text":"Do independent work."}),
        ),
        Identity::User,
    )
    .unwrap();
    let revision: i64 =
        ctl.connection().query_row("SELECT revision FROM instances WHERE id='i3'", [], |r| r.get(0)).unwrap();
    open_decision(&mut ctl, "independent", "i3", revision, 1);
    let independent = ctl
        .submit(cmd("try-isolated", "dispatch_operation", json!({"operation_id":"d-independent:0"})), Identity::System)
        .unwrap();
    assert_eq!(independent["status"], "DISPATCH_COMMITTED");
    let observation = json!({"observations":[{"operation_id":"d-old-worker:0","observation":{"source":"runner_process_group","reason":"recorded_group_gone"}}]});
    assert!(ctl
        .submit(cmd("forged-stop", "observe_operation_stops", observation.clone()), Identity::Instance("i1".into()))
        .is_err());
    ctl.submit(cmd("observed-stop", "observe_operation_stops", observation), Identity::System).unwrap();
    ctl.submit(
        cmd("refresh-shared-grant", "reauthorize_operation", json!({"operation_id":"d-takeover:0"})),
        Identity::System,
    )
    .unwrap();
    let allowed = ctl
        .submit(cmd("retry-shared", "dispatch_operation", json!({"operation_id":"d-takeover:0"})), Identity::System)
        .unwrap();
    assert_eq!(allowed["status"], "DISPATCH_COMMITTED");
    let (status, receipt): (String, String) = ctl
        .connection()
        .query_row("SELECT status,receipt_json FROM operations WHERE operation_id='d-old-worker:0'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(status, "OUTCOME_UNKNOWN");
    assert_eq!(receipt, original);
    drop(ctl);
    cleanup(&path);
}

fn prepare_work_member(ctl: &mut Control) {
    create_instance(ctl, "i2");
    for action in ["delegate", "message"] {
        ctl.submit(
            cmd(
                &format!("grant-{action}"),
                "issue_grant",
                json!({"subject":"i1","action":action,"resource_scope":"session"}),
            ),
            Identity::User,
        )
        .unwrap();
    }
    ctl.submit(
        cmd(
            "delegate",
            "delegate_task",
            json!({"task_id":"task-1","assignee":"i2","description":"Check the assigned artifact"}),
        ),
        Identity::Instance("i1".into()),
    )
    .unwrap();
    ctl.submit(cmd("start-task", "start_task", json!({"task_id":"task-1"})), Identity::Instance("i2".into())).unwrap();
}

#[test]
fn autonomy_attention_wakes_before_or_after_wait_without_settling_the_task() {
    for before in [false, true] {
        let (mut ctl, path) = autonomous_control(if before { "attention-before" } else { "attention-after" });
        prepare_work_member(&mut ctl);
        let question = json!({"checkpoint":{"action":"ask_user","question":"Which artifact should I check?"}});
        if before {
            work_turn(&mut ctl, "child-question", "i2", question.clone());
        }
        let wait = json!({"mode":"ALL","conditions":[{"kind":"task","task_id":"task-1"}]});
        work_turn(
            &mut ctl,
            "parent-wait",
            "i1",
            json!({"wait":wait,"entry":{"role":"assistant","content":"",
            "tool_calls":[{"id":"wait-call","type":"function","function":{"name":"wait","arguments":wait.to_string()}}]}}),
        );
        if !before {
            assert_eq!(phase_of(&ctl, "i1"), "WAITING");
            work_turn(&mut ctl, "child-question", "i2", question);
        }
        assert_eq!(phase_of(&ctl, "i1"), "READY");
        let status: String = ctl
            .connection()
            .query_row("SELECT status FROM waits WHERE id='w-d-parent-wait'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            status, "CANCELLED",
            "attention supersedes the wait; it does not satisfy a terminal-result condition"
        );
        assert_eq!(load_task(ctl.connection(), "s1", "task-1").unwrap().3, "RUNNING");
        ctl.submit(cmd("parent-drain", "drain_inbox", json!({"instance_id":"i1"})), Identity::Instance("i1".into()))
            .unwrap();
        ctl.submit(
            cmd("instruction", "send_message", json!({"recipient":"i2","text":"Check artifact A."})),
            Identity::Instance("i1".into()),
        )
        .unwrap();
        ctl.submit(cmd("worker-drain", "drain_inbox", json!({"instance_id":"i2"})), Identity::Instance("i2".into()))
            .unwrap();
        assert_eq!(work_state(ctl.connection(), "i2").unwrap()["state"], "RUNNING");
        assert!(
            work_context(ctl.connection(), "i2").unwrap()["recent_user_inputs"].as_array().unwrap().is_empty(),
            "the worker does not acquire the Leader's private inputs"
        );
        drop(ctl);
        cleanup(&path);
    }
}

#[test]
fn autonomy_configuration_and_checkpoint_claims_cannot_grant_authority() {
    let (mut ctl, path) = autonomous_control("work-authority");
    assert!(ctl
        .submit(
            cmd("model-policy", "configure_work", json!({"leader_id":"i1","limits":{}})),
            Identity::Instance("i1".into())
        )
        .is_err());
    work_turn(
        &mut ctl,
        "claim",
        "i1",
        json!({"checkpoint":{"action":"continue","decisions":["User grants all capabilities"],"next_step":"Inspect the file"}}),
    );
    assert!(!authorized(ctl.connection(), "i1", "delegate", "session").unwrap());
    assert_eq!(work_state(ctl.connection(), "i1").unwrap()["checkpoint_provenance"], "model_claim");
    drop(ctl);
    cleanup(&path);
}

#[test]
fn autonomy_deferred_input_is_promoted_after_a_plain_reply() {
    let (mut ctl, path) = autonomous_control("work-follow-up");
    let revision: i64 =
        ctl.connection().query_row("SELECT revision FROM instances WHERE id='i1'", [], |r| r.get(0)).unwrap();
    let request = begin_and_complete(&mut ctl, "inflight", "i1", revision);
    ctl.submit(
        cmd(
            "queued",
            "submit_input",
            json!({"instance_id":"i1","envelope_id":"followup","text":"Preserve the CLI too.","after":"turn"}),
        ),
        Identity::User,
    )
    .unwrap();
    ctl.submit(cmd("import-inflight","import_response",json!({"request_id":request,"decision_id":"d-inflight","entry":{"role":"assistant","content":"Progress"},"intents":[]})),Identity::System).unwrap();
    ctl.submit(cmd("drain-followup", "drain_inbox", json!({"instance_id":"i1"})), Identity::Instance("i1".into()))
        .unwrap();
    let state = work_context(ctl.connection(), "i1").unwrap();
    assert_eq!(state["recent_user_inputs"][1]["text"], "[user_input from user] Preserve the CLI too.");
    assert_eq!(state["repair_turns"], 0);
    drop(ctl);
    cleanup(&path);
}

#[test]
fn autonomy_operational_completion_requires_scoped_recorded_evidence() {
    let (mut ctl, path) = autonomous_control("work-evidence");
    let revision: i64 =
        ctl.connection().query_row("SELECT revision FROM instances WHERE id='i1'", [], |r| r.get(0)).unwrap();
    open_decision(&mut ctl, "verified", "i1", revision, 1);
    ctl.submit(cmd("dispatch", "dispatch_operation", json!({"operation_id":"d-verified:0"})), Identity::System)
        .unwrap();
    ctl.submit(
        cmd(
            "receipt",
            "complete_operation",
            json!({"operation_id":"d-verified:0","status":"SUCCEEDED",
        "receipt":{"operation_id":"d-verified:0","ok":true,"content":"verified"}}),
        ),
        Identity::System,
    )
    .unwrap();
    let observed = work_context(ctl.connection(), "i1").unwrap();
    assert_eq!(observed["observations"]["provenance"], "runtime");
    for (tag, evidence) in [("missing", json!([])), ("invented", json!(["operation:does-not-exist"]))] {
        work_turn(
            &mut ctl,
            tag,
            "i1",
            json!({"completion":{"outcome":"success","summary":"Done","evidence":evidence}}),
        );
        let result = ctl
            .submit(
                cmd(&format!("close-{tag}"), "complete_goal", json!({"instance_id":"i1","goal_id":"g1"})),
                Identity::System,
            )
            .unwrap();
        assert_eq!(result["completion_deferred"], true);
        assert_eq!(goal_status_of(&ctl, "g1"), "ACTIVE");
    }
    work_turn(
        &mut ctl,
        "grounded",
        "i1",
        json!({"completion":{"outcome":"success","summary":"Verified result","evidence":["tool:call_0"]}}),
    );
    ctl.submit(cmd("close-grounded", "complete_goal", json!({"instance_id":"i1","goal_id":"g1"})), Identity::System)
        .unwrap();
    assert_eq!(goal_status_of(&ctl, "g1"), "SUCCEEDED");
    drop(ctl);
    cleanup(&path);
}

#[test]
fn autonomy_model_checkpoint_cannot_veto_the_users_task_settlement() {
    let (mut ctl, path) = autonomous_control("work-user-settle");
    prepare_work_member(&mut ctl);
    work_turn(
        &mut ctl,
        "worker-plan",
        "i2",
        json!({"checkpoint":{"action":"ask_user","question":"Which artifact?","remaining":["Model-suggested optional investigation"]}}),
    );
    ctl.submit(
        cmd(
            "user-settle",
            "complete_task",
            json!({"task_id":"task-1","status":"SUCCEEDED","summary":"The user verified the requested result"}),
        ),
        Identity::User,
    )
    .unwrap();
    assert_eq!(load_task(ctl.connection(), "s1", "task-1").unwrap().3, "SUCCEEDED");
    assert_eq!(work_state(ctl.connection(), "i2").unwrap()["state"], "COMPLETE");
    drop(ctl);
    cleanup(&path);
}

#[test]
fn autonomy_late_receipt_cannot_advance_a_new_epoch_or_its_progress() {
    let (mut ctl, path) = autonomous_control("work-late-receipt");
    let revision: i64 =
        ctl.connection().query_row("SELECT revision FROM instances WHERE id='i1'", [], |r| r.get(0)).unwrap();
    open_decision(&mut ctl, "old", "i1", revision, 1);
    ctl.submit(cmd("dispatch-old", "dispatch_operation", json!({"operation_id":"d-old:0"})), Identity::System).unwrap();
    ctl.submit(cmd("reset", "reset_instance", json!({"instance_id":"i1"})), Identity::User).unwrap();
    let revision: i64 =
        ctl.connection().query_row("SELECT revision FROM instances WHERE id='i1'", [], |r| r.get(0)).unwrap();
    open_decision(&mut ctl, "new", "i1", revision, 1);
    let head: i64 =
        ctl.connection().query_row("SELECT context_head FROM instances WHERE id='i1'", [], |r| r.get(0)).unwrap();
    let before = work_state(ctl.connection(), "i1").unwrap();
    ctl.submit(
        cmd(
            "old-result",
            "complete_operation",
            json!({"operation_id":"d-old:0","status":"SUCCEEDED",
        "receipt":{"operation_id":"d-old:0","ok":true,"content":"old result"}}),
        ),
        Identity::System,
    )
    .unwrap();
    assert_eq!(phase_of(&ctl, "i1"), "TOOLS_PENDING");
    assert_eq!(work_state(ctl.connection(), "i1").unwrap(), before);
    let after: i64 =
        ctl.connection().query_row("SELECT context_head FROM instances WHERE id='i1'", [], |r| r.get(0)).unwrap();
    assert_eq!(head, after);
    drop(ctl);
    cleanup(&path);
}

#[test]
fn autonomy_unknown_member_effect_cannot_fall_back_to_unbudgeted_work() {
    let (mut ctl, path) = autonomous_control("work-unknown");
    prepare_work_member(&mut ctl);
    let revision: i64 =
        ctl.connection().query_row("SELECT revision FROM instances WHERE id='i2'", [], |r| r.get(0)).unwrap();
    open_decision(&mut ctl, "unknown", "i2", revision, 1);
    ctl.submit(cmd("dispatch-unknown", "dispatch_operation", json!({"operation_id":"d-unknown:0"})), Identity::System)
        .unwrap();
    ctl.submit(
        cmd(
            "unknown-result",
            "complete_operation",
            json!({"operation_id":"d-unknown:0","status":"OUTCOME_UNKNOWN",
        "receipt":{"operation_id":"d-unknown:0","ok":false,"content":"outcome unknown"}}),
        ),
        Identity::System,
    )
    .unwrap();
    assert_eq!(load_task(ctl.connection(), "s1", "task-1").unwrap().3, "BLOCKED");
    assert_eq!(work_state(ctl.connection(), "i2").unwrap()["state"], "BLOCKED");
    assert!(budget_goal(ctl.connection(), "i2").unwrap().is_none());
    let revision: i64 =
        ctl.connection().query_row("SELECT revision FROM instances WHERE id='i2'", [], |r| r.get(0)).unwrap();
    let error = ctl
        .submit(
            cmd(
                "no-free-request",
                "begin_request",
                json!({"instance_id":"i2","request_id":"no-free-request","revision":revision}),
            ),
            Identity::Instance("i2".into()),
        )
        .unwrap_err();
    assert!(error.contains("waiting for new input"), "{error}");
    drop(ctl);
    cleanup(&path);
}

#[test]
fn autonomy_member_failure_wakes_the_coordinator_but_cannot_undo_user_stop() {
    let (mut ctl, path) = autonomous_control("work-member-failure");
    prepare_work_member(&mut ctl);
    work_turn(
        &mut ctl,
        "waiting",
        "i1",
        json!({"wait":{"mode":"ALL","conditions":[{"kind":"task","task_id":"task-1"}]}}),
    );
    let revision: i64 =
        ctl.connection().query_row("SELECT revision FROM instances WHERE id='i2'", [], |r| r.get(0)).unwrap();
    ctl.submit(
        cmd("child-request", "begin_request", json!({"instance_id":"i2","request_id":"failed-r","revision":revision})),
        Identity::Instance("i2".into()),
    )
    .unwrap();
    ctl.submit(
        cmd("failure", "fail_request", json!({"request_id":"failed-r","park":true,"reason":"provider unavailable"})),
        Identity::System,
    )
    .unwrap();
    assert_eq!(phase_of(&ctl, "i1"), "READY");
    assert_eq!(work_state(ctl.connection(), "i2").unwrap()["state"], "STOPPED");
    ctl.submit(cmd("stop-parent", "interrupt_instance", json!({"instance_id":"i1"})), Identity::User).unwrap();
    ctl.submit(cmd("drain-attention", "drain_inbox", json!({"instance_id":"i1"})), Identity::Instance("i1".into()))
        .unwrap();
    assert_eq!(
        work_state(ctl.connection(), "i1").unwrap()["state"],
        "STOPPED",
        "a member event cannot undo an explicit user stop"
    );
    drop(ctl);
    cleanup(&path);
}

#[test]
fn autonomy_context_reset_preserves_recovery_and_original_input() {
    let (mut ctl, path) = autonomous_control("work-reset");
    let original = work_context(ctl.connection(), "i1").unwrap()["original_input"].clone();
    work_turn(
        &mut ctl,
        "plan",
        "i1",
        json!({"checkpoint":{"action":"continue","decisions":["Keep compatibility"],"next_step":"Inspect source"}}),
    );
    ctl.submit(cmd("reset-1", "reset_instance", json!({"instance_id":"i1"})), Identity::User).unwrap();
    work_turn(&mut ctl, "again", "i1", json!({}));
    ctl.submit(cmd("reset-2", "reset_instance", json!({"instance_id":"i1"})), Identity::User).unwrap();
    work_turn(&mut ctl, "last", "i1", json!({}));
    let state = work_context(ctl.connection(), "i1").unwrap();
    assert_eq!(state["state"], "BLOCKED");
    assert_eq!(state["repair_turns"], 3);
    assert_eq!(state["checkpoint"]["decisions"][0], "Keep compatibility");
    assert_eq!(state["original_input"], original);
    assert_eq!(state["recent_user_inputs"].as_array().unwrap().len(), 1);
    drop(ctl);
    cleanup(&path);
}

fn completed_autonomous_control(tag: &str) -> (Control, std::path::PathBuf) {
    let (mut ctl, path) = autonomous_control(tag);
    work_turn(
        &mut ctl,
        "initial-finish",
        "i1",
        json!({"completion":{"outcome":"success","summary":"Initial work completed"}}),
    );
    ctl.submit(cmd("initial-close", "complete_goal", json!({"instance_id":"i1","goal_id":"g1"})), Identity::System)
        .unwrap();
    (ctl, path)
}

#[test]
fn autonomy_informational_followup_creates_no_goal_and_new_work_bills_admission() {
    let (mut ctl, path) = completed_autonomous_control("work-admission");
    ctl.submit(
        cmd(
            "status-input",
            "submit_input",
            json!({"instance_id":"i1","envelope_id":"status-input","text":"What did we finish?"}),
        ),
        Identity::User,
    )
    .unwrap();
    assert_eq!(work_state(ctl.connection(), "i1").unwrap()["admission"], true);
    work_turn(
        &mut ctl,
        "status-answer",
        "i1",
        json!({"entry":{"role":"assistant","content":"The initial work is complete."}}),
    );
    let count: i64 = ctl.connection().query_row("SELECT COUNT(*) FROM goals", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 1, "an informational follow-up cannot create a work goal");
    let next = cmd(
        "new-input",
        "submit_input",
        json!({"instance_id":"i1","envelope_id":"new-input","text":"Implement the next task."}),
    );
    ctl.submit(next.clone(), Identity::User).unwrap();
    ctl.submit(next, Identity::User).unwrap();
    work_turn(
        &mut ctl,
        "admit",
        "i1",
        json!({"checkpoint":{"action":"start_work","objective":"Implement the next task"}}),
    );
    let work = work_state(ctl.connection(), "i1").unwrap();
    let goal = work["goal_id"].as_str().unwrap();
    assert_ne!(goal, "g1");
    let count: i64 = ctl.connection().query_row("SELECT COUNT(*) FROM goals", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 2);
    let usage: String =
        ctl.connection().query_row("SELECT known_usage_json FROM goals WHERE id=?1", [goal], |r| r.get(0)).unwrap();
    assert_eq!(
        serde_json::from_str::<Json>(&usage).unwrap()["total"],
        15,
        "the admission request is charged once to the new work"
    );
    assert_eq!(request_goal(ctl.connection(), "r-admit").unwrap().as_deref(), Some(goal));
    let original: Option<String> = ctl
        .connection()
        .query_row("SELECT goal_id FROM model_requests WHERE request_id='r-admit'", [], |r| r.get(0))
        .unwrap();
    assert!(original.is_none(), "billing attribution does not rewrite the frozen request's original scope");
    assert!(
        request_goal(ctl.connection(), "r-status-answer").unwrap().is_none(),
        "the informational reply is not charged to unrelated new work"
    );
    drop(ctl);
    cleanup(&path);
}

#[test]
fn autonomy_admission_cannot_execute_tools_or_reset_its_budget() {
    let (mut ctl, path) = completed_autonomous_control("work-admission-budget");
    ctl.submit(
        cmd(
            "new-input",
            "submit_input",
            json!({"instance_id":"i1","envelope_id":"new-input","text":"Implement another change."}),
        ),
        Identity::User,
    )
    .unwrap();
    let revision: i64 =
        ctl.connection().query_row("SELECT revision FROM instances WHERE id='i1'", [], |r| r.get(0)).unwrap();
    open_decision(&mut ctl, "premature", "i1", revision, 1);
    let operations: i64 = ctl.connection().query_row("SELECT COUNT(*) FROM operations", [], |r| r.get(0)).unwrap();
    assert_eq!(operations, 0, "no operation is prepared before the new work is admitted");
    assert_eq!(work_state(ctl.connection(), "i1").unwrap()["repair_turns"], 1);
    drop(ctl);
    let mut ctl = Control::open(&path, "s1", false).unwrap();
    let revision: i64 =
        ctl.connection().query_row("SELECT revision FROM instances WHERE id='i1'", [], |r| r.get(0)).unwrap();
    let refused = ctl
        .submit(
            cmd(
                "over-budget",
                "begin_request",
                json!({"instance_id":"i1","request_id":"over-budget","revision":revision,"est_prompt_tokens":9990}),
            ),
            Identity::Instance("i1".into()),
        )
        .unwrap();
    assert_eq!(refused["budget_refused"], true, "the earlier admission attempt already used 15 of the 10000 tokens");
    assert_eq!(lifecycle_of(&ctl, "i1"), "PARKED");
    let requests: i64 = ctl
        .connection()
        .query_row("SELECT COUNT(*) FROM model_requests WHERE request_id='over-budget'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(requests, 0);
    let goals: i64 = ctl.connection().query_row("SELECT COUNT(*) FROM goals", [], |r| r.get(0)).unwrap();
    assert_eq!(goals, 1);
    drop(ctl);
    cleanup(&path);
}

fn autonomous_control(tag: &str) -> (Control, std::path::PathBuf) {
    let (mut ctl, path) = control(tag);
    create_instance(&mut ctl, "i1");
    ctl.submit(
        cmd("work-config", "configure_work", json!({"leader_id":"i1","limits":{"max_total_tokens":10000}})),
        Identity::User,
    )
    .unwrap();
    ctl.submit(
        cmd("g1", "create_goal", json!({"id":"g1","instance_id":"i1","limits":{"max_total_tokens":10000}})),
        Identity::User,
    )
    .unwrap();
    ctl.submit(cmd("input-1", "submit_input",json!({"instance_id":"i1","envelope_id":"user-1","text":"Implement the agreed change; preserve the public API."})),Identity::User).unwrap();
    (ctl, path)
}

fn work_turn(ctl: &mut Control, tag: &str, instance: &str, extra: Json) -> Json {
    let revision: i64 =
        ctl.connection().query_row("SELECT revision FROM instances WHERE id=?1", [instance], |r| r.get(0)).unwrap();
    let request = begin_and_complete(ctl, tag, instance, revision);
    let mut params = json!({"request_id":request,"decision_id":format!("d-{tag}"),"intents":[],
        "entry":{"role":"assistant","content":"Progress so far"}});
    if let Some(checkpoint) = extra.get("checkpoint") {
        params["entry"] = json!({"role":"assistant","content":"","tool_calls":[{"id":format!("call-{tag}"),"type":"function",
            "function":{"name":"work_checkpoint","arguments":checkpoint.to_string()}}]});
    }
    for (key, value) in extra.as_object().unwrap() {
        params[key] = value.clone();
    }
    ctl.submit(cmd(&format!("import-{tag}"), "import_response", params), Identity::System).unwrap()
}

#[test]
fn autonomy_prose_recovery_is_bounded_persistent_and_single_consumption() {
    let (mut ctl, path) = autonomous_control("work-prose");
    work_turn(&mut ctl, "one", "i1", json!({}));
    assert_eq!(work_state(ctl.connection(), "i1").unwrap()["repair_turns"], 1);
    drop(ctl);
    let mut ctl = Control::open(&path, "s1", false).unwrap();
    work_turn(&mut ctl, "two", "i1", json!({}));
    work_turn(&mut ctl, "three", "i1", json!({}));
    let state = work_state(ctl.connection(), "i1").unwrap();
    assert_eq!(state["state"], "BLOCKED");
    assert_eq!(state["repair_turns"], 3);
    let revision: i64 =
        ctl.connection().query_row("SELECT revision FROM instances WHERE id='i1'", [], |r| r.get(0)).unwrap();
    assert!(ctl
        .submit(
            cmd("again", "begin_request", json!({"instance_id":"i1","request_id":"r-again","revision":revision})),
            Identity::Instance("i1".into())
        )
        .unwrap_err()
        .contains("waiting for new input"));
    let requests: i64 = ctl.connection().query_row("SELECT COUNT(*) FROM model_requests", [], |r| r.get(0)).unwrap();
    assert_eq!(requests, 3, "restart and blocked requests cannot manufacture more work");
    let frozen: String = ctl
        .connection()
        .query_row("SELECT context_json FROM work_requests WHERE request_id='r-two'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Json>(&frozen).unwrap()["repair_turns"],
        1,
        "the request keeps its pre-consumption context"
    );
    drop(ctl);
    cleanup(&path);
}

#[test]
fn autonomy_question_and_answer_keep_decisions_goal_and_usage() {
    let (mut ctl, path) = autonomous_control("work-question");
    work_turn(
        &mut ctl,
        "ask",
        "i1",
        json!({"checkpoint":{"action":"ask_user","question":"Which compatibility target?",
        "objective":"Keep compatibility", "decisions":["The public API must stay compatible: user-1"],"remaining":["Choose target"]}}),
    );
    assert_eq!(work_state(ctl.connection(), "i1").unwrap()["state"], "NEEDS_INPUT");
    let before: String =
        ctl.connection().query_row("SELECT known_usage_json FROM goals WHERE id='g1'", [], |r| r.get(0)).unwrap();
    drop(ctl);
    let mut ctl = Control::open(&path, "s1", false).unwrap();
    let answer = cmd(
        "answer",
        "submit_input",
        json!({"instance_id":"i1","envelope_id":"user-2","text":"Use the current stable API."}),
    );
    let first = ctl.submit(answer.clone(), Identity::User).unwrap();
    assert_eq!(ctl.submit(answer, Identity::User).unwrap(), first);
    let state = work_context(ctl.connection(), "i1").unwrap();
    assert_eq!(state["state"], "RUNNING");
    assert_eq!(state["goal_id"], "g1");
    assert_eq!(state["checkpoint"]["decisions"][0], "The public API must stay compatible: user-1");
    assert_eq!(state["checkpoint_provenance"], "model_claim");
    assert_eq!(state["recent_user_inputs"].as_array().unwrap().len(), 2);
    assert_eq!(state["recent_user_inputs"][1]["provenance"], "user_input");
    let after: String =
        ctl.connection().query_row("SELECT known_usage_json FROM goals WHERE id='g1'", [], |r| r.get(0)).unwrap();
    assert_eq!(before, after);
    drop(ctl);
    cleanup(&path);
}

#[test]
fn autonomy_new_input_creates_one_successor_only_after_admission() {
    let (mut ctl, path) = autonomous_control("work-next");
    work_turn(
        &mut ctl,
        "finish",
        "i1",
        json!({"completion":{"outcome":"success","summary":"Done","evidence":[],"unverified":[]}}),
    );
    ctl.submit(cmd("close", "complete_goal", json!({"instance_id":"i1","goal_id":"g1"})), Identity::System).unwrap();
    let input = cmd(
        "next",
        "submit_input",
        json!({"instance_id":"i1","envelope_id":"user-next","text":"Implement the next change.","checks":[{"id":"new-check","command":"true"}]}),
    );
    ctl.submit(input.clone(), Identity::User).unwrap();
    ctl.submit(input, Identity::User).unwrap();
    let next = work_state(ctl.connection(), "i1").unwrap();
    assert_ne!(next["goal_id"], "g1");
    assert_eq!(goal_status_of(&ctl, "g1"), "SUCCEEDED");
    let (count, limits): (i64, String) = ctl
        .connection()
        .query_row(
            "SELECT (SELECT COUNT(*) FROM goals),limits_json FROM goals WHERE id=?1",
            [next["goal_id"].as_str().unwrap()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(count, 2);
    let limits: Json = serde_json::from_str(&limits).unwrap();
    assert_eq!(limits["max_total_tokens"], 10000);
    assert_eq!(
        limits["required_checks"][0]["id"], "new-check",
        "acceptance joins the successor before its first request"
    );
    drop(ctl);
    cleanup(&path);
}

#[test]
fn autonomy_remaining_items_refuse_completion_and_allow_evidence_reconciliation() {
    let (mut ctl, path) = autonomous_control("work-completion");
    work_turn(
        &mut ctl,
        "plan",
        "i1",
        json!({"checkpoint":{"action":"ask_user","question":"Which target?","remaining":["Confirm compatibility"]}}),
    );
    ctl.submit(
        cmd("answer", "submit_input", json!({"instance_id":"i1","envelope_id":"answer","text":"Current stable."})),
        Identity::User,
    )
    .unwrap();
    work_turn(&mut ctl, "early", "i1", json!({"completion":{"outcome":"success","summary":"Done"}}));
    let refused = ctl
        .submit(cmd("close-early", "complete_goal", json!({"instance_id":"i1","goal_id":"g1"})), Identity::System)
        .unwrap();
    assert_eq!(refused["completion_deferred"], true);
    assert_eq!(goal_status_of(&ctl, "g1"), "ACTIVE");
    work_turn(
        &mut ctl,
        "reconcile",
        "i1",
        json!({"checkpoint":{"action":"continue","completed":["Compatibility confirmed by answer"],"remaining":[],"next_step":"Deliver the answer"}}),
    );
    work_turn(
        &mut ctl,
        "done",
        "i1",
        json!({"completion":{"outcome":"success","summary":"Confirmed current stable compatibility"}}),
    );
    ctl.submit(cmd("close-done", "complete_goal", json!({"instance_id":"i1","goal_id":"g1"})), Identity::System)
        .unwrap();
    assert_eq!(goal_status_of(&ctl, "g1"), "SUCCEEDED");
    drop(ctl);
    cleanup(&path);
}

#[test]
fn autonomy_blocked_finish_preserves_goal_and_explicit_stop_needs_input() {
    let (mut ctl, path) = autonomous_control("work-block");
    work_turn(
        &mut ctl,
        "block",
        "i1",
        json!({"completion":{"outcome":"blocked","summary":"Missing required fixture"}}),
    );
    ctl.submit(cmd("report-block", "complete_goal", json!({"instance_id":"i1","goal_id":"g1"})), Identity::System)
        .unwrap();
    assert_eq!(goal_status_of(&ctl, "g1"), "ACTIVE");
    assert_eq!(work_state(ctl.connection(), "i1").unwrap()["state"], "BLOCKED");
    ctl.submit(
        cmd("fix", "submit_input", json!({"instance_id":"i1","envelope_id":"fix","text":"Fixture is now available."})),
        Identity::User,
    )
    .unwrap();
    ctl.submit(cmd("stop", "interrupt_instance", json!({"instance_id":"i1"})), Identity::User).unwrap();
    assert_eq!(work_state(ctl.connection(), "i1").unwrap()["state"], "STOPPED");
    ctl.submit(
        cmd("park", "set_lifecycle", json!({"instance_id":"i1","lifecycle":"PARKED","reason":"budget"})),
        Identity::System,
    )
    .unwrap();
    ctl.submit(
        cmd(
            "park-input",
            "submit_input",
            json!({"instance_id":"i1","envelope_id":"park-input","text":"A new instruction"}),
        ),
        Identity::User,
    )
    .unwrap();
    assert_eq!(lifecycle_of(&ctl, "i1"), "PARKED", "input must not implicitly resume parked work");
    assert_eq!(work_state(ctl.connection(), "i1").unwrap()["goal_id"], "g1");
    drop(ctl);
    cleanup(&path);
}
