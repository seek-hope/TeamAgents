fn autonomy_runner_child(name: &str) -> bool {
    if std::env::var("TA_AUTONOMY_TEST_CHILD").as_deref() == Ok(name) {
        return true;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env("TA_AUTONOMY_TEST_CHILD", name)
        .env("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"))
        .output()
        .unwrap();
    assert!(
        output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed; 0 ignored"),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

fn work_checkpoint(id: &str, value: Json) -> Json {
    json!({"role":"assistant","content":"","tool_calls":[{"id":id,"type":"function",
        "function":{"name":"work_checkpoint","arguments":value.to_string()}}]})
}

fn autonomous_config<P: Provider + 'static>(root: &Root, provider: P) -> DriverConfig<P> {
    let mut config = root.config_with(provider);
    config.goal_limits = json!({"autonomous_work":true});
    config
}

#[tokio::test]
async fn autonomy_shared_takeover_waits_for_real_stop_and_preserves_unknown_receipt() {
    if !autonomy_runner_child("autonomy_shared_takeover_waits_for_real_stop_and_preserves_unknown_receipt") {
        eprintln!("Completed in the isolated child runner; its exit and test count were checked.");
        return;
    }
    use std::os::unix::process::CommandExt;
    use teamagents_core::v2::{Command, Control, Identity};
    use teamagents_engine::jobs::{self, JobSpec, Journal};
    struct Process(std::process::Child);
    impl Drop for Process {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn submit(ctl: &mut Control, method: &str, params: Json, identity: Identity) -> Json {
        ctl.submit(Command { command_id: uuid::Uuid::new_v4().to_string(), method: method.into(), params }, identity)
            .unwrap()
    }
    let root = root("autonomy-takeover");
    let workspace = root.dir.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let mut process = Process(std::process::Command::new("sleep").arg("60").process_group(0).spawn().unwrap());
    let job_dir = root.dir.join("state/instances/i-old/jobs/old-decision_0");
    std::fs::create_dir_all(&job_dir).unwrap();
    let job = JobSpec {
        job_id: "old-decision:0".into(),
        program: "/bin/bash".into(),
        args: vec!["-lc".into(), "sleep 60".into()],
        cwd: workspace.to_string_lossy().into_owned(),
        env: vec![],
        deadline_ms: 0,
        token: "test-token".into(),
    };
    let journal = Journal {
        job_id: job.job_id.clone(),
        state: "OUTCOME_UNKNOWN".into(),
        command_hash: job.command_hash(),
        pid: Some(process.0.id()),
        start_ticks: Some(jobs::start_ticks(process.0.id()).unwrap()),
        boot_id: jobs::boot_id().unwrap(),
        exit_code: None,
        signal: None,
        started_ms: Some(jobs::now_ms()),
        finished_ms: None,
        starts: 1,
        cancel_saved: true,
    };
    jobs::atomic_json(&job_dir.join("job.json"), &job).unwrap();
    jobs::atomic_json(&job_dir.join("journal.json"), &journal).unwrap();
    let journal_before = std::fs::read(job_dir.join("journal.json")).unwrap();
    let (provider, seen) = recording(vec![
        Step::Message(shell_call("before-stop", "printf integrated > takeover.txt")),
        Step::Message(work_checkpoint(
            "blocked",
            json!({"action":"blocked","reason":"Waiting for the conflicting process to stop"}),
        )),
        Step::Message(shell_call("after-stop", "printf integrated > takeover.txt")),
        Step::Message(
            json!({"role":"assistant","content":"","tool_calls":[{"id":"finish-takeover","type":"function","function":{"name":"finish",
            "arguments":json!({"status":"success","summary":"Takeover completed after the recorded process group stopped","evidence":["tool:after-stop"]}).to_string()}}]}),
        ),
    ]);
    let handle = start(autonomous_config(&root, provider)).await.unwrap();
    let receipt_before=handle.with_control(move |ctl| {
        submit(ctl,"set_lifecycle",json!({"instance_id":"i-main","lifecycle":"PAUSED"}),Identity::User);
        submit(ctl,"submit_input",json!({"instance_id":"i-main","envelope_id":"takeover-input","text":"Take over the cancelled work and verify the result"}),Identity::User);
        submit(ctl,"create_instance",json!({"id":"i-old","workspace_ref":workspace}),Identity::User);
        submit(ctl,"issue_grant",json!({"subject":"i-main","action":"delegate","resource_scope":"session"}),Identity::User);
        submit(ctl,"delegate_task",json!({"task_id":"old-task","assignee":"i-old","description":"Earlier work"}),Identity::Instance("i-main".into()));
        submit(ctl,"start_task",json!({"task_id":"old-task"}),Identity::Instance("i-old".into()));
        let revision:i64=ctl.connection().query_row("SELECT revision FROM instances WHERE id='i-old'",[],|r|r.get(0)).unwrap();
        submit(ctl,"begin_request",json!({"instance_id":"i-old","request_id":"old-request","revision":revision}),Identity::Instance("i-old".into()));
        submit(ctl,"record_attempt",json!({"request_id":"old-request","attempt_id":"old-attempt","status":"COMPLETE","usage":{"prompt_tokens":1,"total_tokens":1}}),Identity::System);
        submit(ctl,"import_response",json!({"request_id":"old-request","decision_id":"old-decision","entry":shell_call("old-call","sleep 60"),
            "intents":[{"index":0,"call_id":"old-call","name":"shell","args":{"command":"sleep 60"}}]}),Identity::System);
        let dispatched=submit(ctl,"dispatch_operation",json!({"operation_id":"old-decision:0"}),Identity::System);
        assert_eq!(dispatched["status"],"DISPATCH_COMMITTED");
        submit(ctl,"cancel_task",json!({"task_id":"old-task"}),Identity::User);
        submit(ctl,"complete_operation",json!({"operation_id":"old-decision:0","status":"OUTCOME_UNKNOWN",
            "receipt":{"operation_id":"old-decision:0","started":true,"content":"outcome is unknown"}}),Identity::System);
        let receipt:String=ctl.connection().query_row("SELECT receipt_json FROM operations WHERE operation_id='old-decision:0'",[],|r|r.get(0)).unwrap();
        submit(ctl,"set_lifecycle",json!({"instance_id":"i-main","lifecycle":"ACTIVE"}),Identity::User);
        receipt
    }).await.unwrap();
    wait_event(&handle, "work_checkpoint", 5000).await;
    assert_eq!(recorded(&seen).len(), 2);
    assert!(!root.dir.join("ws/takeover.txt").exists(), "the conflicting dispatch must not start");
    assert!(process.0.try_wait().unwrap().is_none(), "observation cannot stop the original process");
    process.0.kill().unwrap();
    process.0.wait().unwrap();
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    assert_eq!(std::fs::read_to_string(root.dir.join("ws/takeover.txt")).unwrap(), "integrated");
    assert_eq!(recorded(&seen).len(), 4);
    let preserved = handle
        .with_control(move |ctl| {
            let (status, receipt, stop): (String, String, Option<String>) = ctl
                .connection()
                .query_row(
                    "SELECT status,receipt_json,quiescence_json FROM operations WHERE operation_id='old-decision:0'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            status == "OUTCOME_UNKNOWN" && receipt == receipt_before && stop.is_some()
        })
        .await
        .unwrap();
    assert!(preserved, "stop evidence is separate from the unknown outcome");
    assert_eq!(std::fs::read(job_dir.join("journal.json")).unwrap(), journal_before);
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn autonomy_continues_after_prose_and_verifies_the_written_result() {
    if !autonomy_runner_child("autonomy_continues_after_prose_and_verifies_the_written_result") {
        eprintln!("Completed in the isolated child runner; its exit and test count were checked.");
        return;
    }
    let root = root("autonomy-work");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let (provider, seen) = recording(vec![
        Step::Message(reply("I have identified the change and will implement it.")),
        Step::Message(work_checkpoint(
            "plan",
            json!({"action":"continue","objective":"Write and verify the result",
            "remaining":["Write result"],"next_step":"Write result and check its bytes"}),
        )),
        Step::Message(shell_call("write", "printf 'verified' > result.txt && test \"$(cat result.txt)\" = verified")),
        Step::Message(work_checkpoint(
            "evidence",
            json!({"action":"continue","completed":["result.txt verified by the shell check"],
            "remaining":[],"evidence":["write tool receipt"],"next_step":"Deliver the result"}),
        )),
        Step::Message(json!({"role":"assistant","content":"","tool_calls":[{"id":"finish","type":"function",
            "function":{"name":"finish","arguments":json!({"status":"success","summary":"Wrote and verified result.txt.","evidence":["tool:write"]}).to_string()}}]})),
    ]);
    let handle = start(autonomous_config(&root, provider)).await.unwrap();
    handle.input("Write the result and verify it.").await.unwrap();
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    assert_eq!(std::fs::read_to_string(root.dir.join("ws/result.txt")).unwrap(), "verified");
    let requests = recorded(&seen);
    assert_eq!(requests.len(), 5);
    assert!(requests.iter().all(|r| request_text(r).contains("Behavior policy v1.")));
    assert!(requests.iter().all(|r| r["tool_names"].as_array().unwrap().iter().any(|t| t == "work_checkpoint")));
    assert!(!request_text(&requests[0]).contains("never end a turn leaving it open"));
    assert!(request_text(&requests[4]).contains("result.txt verified by the shell check"));
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn autonomy_question_survives_restart_without_another_confirmation() {
    let root = root("autonomy-question");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let (provider, seen) = recording(vec![Step::Message(work_checkpoint(
        "question",
        json!({"action":"ask_user",
        "question":"Which API compatibility target?","decisions":["Keep the public API compatible"],"objective":"Confirm compatibility"}),
    ))]);
    let handle = start(autonomous_config(&root, provider)).await.unwrap();
    handle.input("Confirm the intended compatibility target.").await.unwrap();
    wait_event(&handle, "work_checkpoint", 5000).await;
    assert_eq!(recorded(&seen).len(), 1);
    handle.shutdown().await.unwrap();
    let (provider, resumed) =
        recording(vec![Step::Message(finish_call("Current stable API compatibility confirmed."))]);
    let handle = start(autonomous_config(&root, provider)).await.unwrap();
    let saved = handle
        .with_control(|ctl| teamagents_core::v2::control::work_state(ctl.connection(), "i-main"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved["state"], "NEEDS_INPUT");
    handle.input("Use the current stable API.").await.unwrap();
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    let requests = recorded(&resumed);
    assert_eq!(requests.len(), 1, "the answer must not trigger another clarification ritual");
    assert!(request_text(&requests[0]).contains("Keep the public API compatible"));
    assert!(request_text(&requests[0]).contains("Use the current stable API."));
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn autonomy_recovery_budget_survives_an_inflight_crash() {
    let root = root("autonomy-crash");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let (provider, seen) =
        recording(vec![Step::Message(reply("I will continue")), Step::Message(reply("Still planning")), Step::Hang]);
    let handle = start(autonomous_config(&root, provider)).await.unwrap();
    handle.input("Complete the work.").await.unwrap();
    wait_for_requests(&seen, 3, 5000).await;
    handle.crash().await;
    let (provider, resumed) =
        recording(vec![Step::Message(reply("More planning")), Step::Message(finish_call("Must not run"))]);
    let handle = start(autonomous_config(&root, provider)).await.unwrap();
    wait_event(&handle, "work_blocked", 5000).await;
    assert_eq!(recorded(&resumed).len(), 1, "restart does not replenish empty-turn recovery");
    let state = handle
        .with_control(|ctl| teamagents_core::v2::control::work_state(ctl.connection(), "i-main"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state["repair_turns"], 3);
    assert_eq!(state["state"], "BLOCKED");
    assert_eq!(handle.snapshot().await.unwrap()["goal"]["status"], "ACTIVE");
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn autonomy_repeated_actions_stop_without_inventing_progress() {
    if !autonomy_runner_child("autonomy_repeated_actions_stop_without_inventing_progress") {
        eprintln!("Completed in the isolated child runner; its exit and test count were checked.");
        return;
    }
    let root = root("autonomy-repeat");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let script = (0..6).map(|n| Step::Message(shell_call(&format!("read-{n}"), "printf unchanged"))).collect();
    let (provider, seen) = recording(script);
    let handle = start(autonomous_config(&root, provider)).await.unwrap();
    handle.input("Investigate and resolve the issue.").await.unwrap();
    wait_event(&handle, "work_blocked", 15000).await;
    assert_eq!(recorded(&seen).len(), 5, "three equal observations plus two bounded recovery attempts");
    let events = handle.events(0).await.unwrap();
    assert!(!events.iter().any(|e| e["kind"] == "goal_completed"));
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn autonomy_wait_sleeps_until_its_registered_event_then_continues() {
    let root = root("autonomy-wait");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let (provider, seen) = recording(vec![
        Step::Message(wait_call(
            "wait",
            json!({"mode":"ANY",
        "conditions":[{"kind":"message","from":"i-peer"}],"timer_seconds":1}),
        )),
        Step::Message(finish_call("Wait completed.")),
    ]);
    let handle = start(autonomous_config(&root, provider)).await.unwrap();
    handle.input("Wait for the peer or the deadline.").await.unwrap();
    wait_phase(&handle, "WAITING", 5000).await;
    assert_eq!(recorded(&seen).len(), 1, "waiting must not poll the model");
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    assert_eq!(recorded(&seen).len(), 2);
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn autonomy_compaction_retains_user_references_and_decisions() {
    let root = root("autonomy-compact");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let bulk = format!("Original specification: preserve compatibility. {}", "padding ".repeat(4000));
    let (provider, seen) = recording(vec![
        Step::Message(work_checkpoint(
            "question",
            json!({"action":"ask_user","question":"Which target?","decisions":["Preserve compatibility"]}),
        )),
        Step::Message(reply(
            "[Compacted conversation summary] The user requires compatibility; target answer follows.",
        )),
        Step::Message(finish_call("Confirmed the stable compatibility target.")),
    ]);
    let mut config = autonomous_config(&root, provider);
    // This provider is scripted; no real model window is changed or used.
    config.profile.context_window = Some(8000);
    let handle = start(config).await.unwrap();
    handle.input(&bulk).await.unwrap();
    wait_event(&handle, "work_checkpoint", 5000).await;
    handle.input("Use stable.").await.unwrap();
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    let requests = recorded(&seen);
    assert_eq!(requests.len(), 3, "one compaction plus the question and final answer");
    assert_eq!(requests[1]["tools"], 0);
    let final_context = request_text(&requests[2]);
    assert!(final_context.contains("Compacted conversation summary"));
    assert!(final_context.contains("Preserve compatibility"));
    assert!(final_context.contains("original_input"));
    assert!(final_context.contains("Use stable."));
    assert!(!final_context.contains(&bulk));
    assert_eq!(
        handle.snapshot().await.unwrap()["goal"]["known_usage"]["total"],
        15,
        "compaction remains charged to the original goal"
    );
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn autonomy_required_checks_still_repair_before_goal_completion() {
    if !autonomy_runner_child("autonomy_required_checks_still_repair_before_goal_completion") {
        eprintln!("Completed in the isolated child runner; its exit and test count were checked.");
        return;
    }
    let root = root("autonomy-checks");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let (provider, _) = recording(vec![
        Step::Message(finish_call("Premature claim")),
        Step::Message(shell_call("fix", "printf done > required.txt")),
        Step::Message(finish_call("Check repaired")),
    ]);
    let mut config = autonomous_config(&root, provider);
    config.goal_limits["required_checks"] = json!([{"id":"result","command":"test -f required.txt"}]);
    let handle = start(config).await.unwrap();
    handle.input("Create required.txt and pass its check.").await.unwrap();
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    assert!(root.dir.join("ws/required.txt").exists());
    handle.shutdown().await.unwrap();
}
