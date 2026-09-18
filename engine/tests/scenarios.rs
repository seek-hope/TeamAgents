//! T1–T5/T9 scenarios, driven through the real core + runtime + scripted members.

mod support;

use serde_json::{json, Value as Json};
use support::*;
use teamagents_core::models::{TaskStatus, TurnRun, TurnStatus};

fn runs(state: &Json) -> Vec<TurnRun> {
    serde_json::from_value(state.get("runs").cloned().unwrap_or(Json::Null)).unwrap_or_default()
}

fn task_status(state: &Json, assignee: &str) -> Option<TaskStatus> {
    state
        .get("tasks")?
        .as_array()?
        .iter()
        .find(|t| t.get("assignee").and_then(|v| v.as_str()) == Some(assignee))
        .and_then(|t| t.get("status").cloned())
        .and_then(|v| serde_json::from_value(v).ok())
}

#[test]
fn t1_delegation_and_summary_full_lifecycle() {
    isolated_state_home("t1");
    let spec = json!({
        "leader_id": "leader",
        "agents": [member("leader", "leader"), member("b", "worker")],
        "channels": [task_channel("leader", &["b"]), message_channel("b", &["leader"])],
    });
    let core = core_with_spec("s1", spec);
    let leader = scripted(
        "leader",
        &json!([
            ["call", "assign_task", {"assignee": "b", "description": "write the report", "acceptance": "report.md exists"}],
            ["call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}],
            ["wait"],
            ["call", "signal_done", {"summary": "report delivered"}],
            ["end"],
        ]),
        barriers(),
    );
    let worker = scripted(
        "b",
        &json!([
            ["call", "complete_task", {"task_id": "$inbox0.payload.task_id", "result_refs": ["artifacts/report.md"], "summary": "wrote report"}],
            ["end"],
        ]),
        barriers(),
    );
    let h = harness_with(core.clone(), vec![("leader", leader), ("b", worker)]);
    h.runtime.start();
    let receipt = h.runtime.user_message("please produce the report", false).unwrap();
    assert!(receipt.ok);
    assert!(h.runtime.settle(10), "runtime settles");

    let state = h.runtime.state().unwrap();
    let tasks = state.get("tasks").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert_eq!(task.get("status").and_then(|v| v.as_str()), Some("SUCCEEDED"));
    assert_eq!(task.get("result_refs").cloned().unwrap_or(Json::Null), json!(["artifacts/report.md"]));
    assert_eq!(task.get("requester").and_then(|v| v.as_str()), Some("leader"));
    assert_eq!(task.get("assignee").and_then(|v| v.as_str()), Some("b"));

    let events: Vec<Json> = state.get("events").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let kinds: Vec<String> =
        events.iter().filter_map(|e| e.get("kind").and_then(|v| v.as_str()).map(str::to_string)).collect();
    for expected in ["task_created", "task_started", "task_completed", "goal_done"] {
        assert!(kinds.contains(&expected.to_string()), "{expected} missing from {kinds:?}");
    }

    // result receipt goes to the requester (Leader) via inbox delivery
    let completed = events.iter().find(|e| e.get("kind").and_then(|v| v.as_str()) == Some("task_completed")).unwrap();
    assert!(completed.get("audience").map(|a| a.to_string().contains("leader")).unwrap_or(false));
    assert_eq!(state.get("session").and_then(|s| s.get("goal_state")).and_then(|v| v.as_str()), Some("done"));
    h.runtime.close();
}

#[test]
fn t9_baseline_leader_alone_executes_and_keeps_talking() {
    isolated_state_home("t9");
    let core = core_with_spec("s2", json!({"leader_id": "leader", "agents": [member("leader", "leader")]}));
    let leader =
        scripted("leader", &json!([["call", "signal_done", {"summary": "answered directly"}], ["end"]]), barriers());
    let h = harness_with(core.clone(), vec![("leader", leader.clone())]);
    h.runtime.start();
    h.runtime.user_message("say hello", false).unwrap();
    assert!(h.runtime.settle(10));
    assert_eq!(
        h.runtime.state().unwrap().get("session").and_then(|s| s.get("goal_state")).and_then(|v| v.as_str()),
        Some("done")
    );

    // second conversation: new goal, same team
    leader.reset(vec![
        teamagents_engine::scripted::Step::Call("signal_done".into(), json!({"summary": "second"})),
        teamagents_engine::scripted::Step::End,
    ]);
    h.runtime.user_message("second question", false).unwrap();
    assert!(h.runtime.settle(10));
    assert_eq!(
        h.runtime.state().unwrap().get("session").and_then(|s| s.get("goal_state")).and_then(|v| v.as_str()),
        Some("done")
    );
    h.runtime.close();
}

#[test]
fn t2_parallel_members_and_mid_run_supplement() {
    isolated_state_home("t2");
    let spec = json!({
        "leader_id": "leader",
        "agents": [member("leader", "leader"), member("b", "worker"), member("c", "worker")],
        "channels": [task_channel("leader", &["b", "c"]), message_channel("b", &["leader"]), message_channel("c", &["leader"])],
    });
    let core = core_with_spec("t2", spec);
    let shared = barriers();
    let leader = scripted(
        "leader",
        &json!([
            ["call", "assign_task", {"assignee": "b", "description": "slow job B"}],
            ["call", "assign_task", {"assignee": "c", "description": "slow job C"}],
            ["call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id", "$r1.result.task_id"]}],
            ["wait"],
            ["inbox"],
            ["call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id", "$r1.result.task_id"]}],
            ["wait"],
            ["inbox"],
            ["call", "signal_done", {"summary": "both done"}],
            ["end"],
        ]),
        shared.clone(),
    );
    let b = scripted(
        "b",
        &json!([
            ["barrier", "both-started"],
            ["sleep", 0.6],
            ["call", "complete_task", {"task_id": "$inbox0.payload.task_id", "result_refs": ["b.out"]}],
            ["end"],
        ]),
        shared.clone(),
    );
    let c = scripted(
        "c",
        &json!([
            ["barrier", "both-started"],
            ["sleep", 0.6],
            ["call", "complete_task", {"task_id": "$inbox0.payload.task_id", "result_refs": ["c.out"]}],
            ["end"],
        ]),
        shared.clone(),
    );
    let h = harness_with(core.clone(), vec![("leader", leader.clone()), ("b", b), ("c", c)]);
    h.runtime.start();
    h.runtime.user_message("run two jobs in parallel", false).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    // while B/C work, a supplement must reach the Leader without waiting for them
    h.runtime.user_message("by the way, also check the logs", true).unwrap();
    assert!(
        wait_for(
            || leader
                .observed_inbox
                .lock()
                .unwrap()
                .iter()
                .any(|i| i.get("kind").and_then(|v| v.as_str()) == Some("user_message")),
            5000
        ),
        "leader never processed the supplement"
    );
    let state = h.runtime.state().unwrap();
    let by_agent: std::collections::HashMap<String, TurnStatus> =
        runs(&state).into_iter().map(|r| (r.agent_id, r.status)).collect();
    assert_eq!(by_agent.get("b"), Some(&TurnStatus::Running));
    assert_eq!(by_agent.get("c"), Some(&TurnStatus::Running));

    assert!(h.runtime.settle(10));
    let final_state = h.runtime.state().unwrap();
    assert_eq!(task_status(&final_state, "b"), Some(TaskStatus::Succeeded));
    assert_eq!(task_status(&final_state, "c"), Some(TaskStatus::Succeeded));
    assert_eq!(final_state.get("session").and_then(|s| s.get("goal_state")).and_then(|v| v.as_str()), Some("done"));
    h.runtime.close();
}

#[test]
fn t3_channel_enforcement_and_exactly_once_delivery() {
    isolated_state_home("t3");
    let spec = json!({
        "leader_id": "leader",
        "agents": [member("leader", "leader"), member("b", "worker"), member("c", "worker"), member("d", "worker")],
        "channels": [
            task_channel("leader", &["b", "c"]),
            message_channel("b", &["c"]), message_channel("c", &["b"]),
            message_channel("b", &["leader"]), message_channel("c", &["leader"]),
        ],
    });
    let core = core_with_spec("t3", spec);
    let leader = scripted(
        "leader",
        &json!([
            ["call", "assign_task", {"assignee": "b", "description": "discuss"}],
            ["call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}],
            ["wait"],
            ["call", "signal_done", {}],
            ["end"],
        ]),
        barriers(),
    );
    let b = scripted(
        "b",
        &json!([
            ["call", "send_message", {"target": "c", "text": "ping"}],
            ["call", "complete_task", {"task_id": "$inbox0.payload.task_id"}],
            ["end"],
            ["inbox"],
            ["end"],
        ]),
        barriers(),
    );
    let c = scripted(
        "c",
        &json!([
            ["inbox"],
            ["call", "send_message", {"target": "b", "text": "pong"}],
            ["call", "send_message", {"target": "d", "text": "leak"}],
            ["end"],
        ]),
        barriers(),
    );
    let d = scripted("d", &json!([]), barriers());
    let h = harness_with(core.clone(), vec![("leader", leader), ("b", b.clone()), ("c", c.clone()), ("d", d)]);
    h.runtime.start();
    h.runtime.user_message("let b and c discuss", false).unwrap();
    assert!(h.runtime.settle(10));

    let c_messages: Vec<String> = c
        .observed_inbox
        .lock()
        .unwrap()
        .iter()
        .filter(|i| i.get("kind").and_then(|v| v.as_str()) == Some("message"))
        .filter_map(|i| i.get("payload")?.get("text")?.as_str().map(str::to_string))
        .collect();
    assert_eq!(c_messages, vec!["ping"]);
    let b_messages: Vec<String> = b
        .observed_inbox
        .lock()
        .unwrap()
        .iter()
        .filter(|i| i.get("kind").and_then(|v| v.as_str()) == Some("message"))
        .filter_map(|i| i.get("payload")?.get("text")?.as_str().map(str::to_string))
        .collect();
    assert_eq!(b_messages, vec!["pong"]);
    let leaks = c
        .results
        .lock()
        .unwrap()
        .iter()
        .filter(|r| {
            r.get("ok").and_then(|v| v.as_bool()) == Some(false)
                && r.get("error").and_then(|v| v.as_str()).unwrap_or("").contains("not allowed to message")
        })
        .count();
    assert!(leaks > 0, "unauthorized send must be refused");
    h.runtime.close();
}

#[test]
fn t4_observer_scoped_events_without_extra_rights() {
    isolated_state_home("t4");
    let spec = json!({
        "leader_id": "leader",
        "agents": [member("leader", "leader"), member("b", "worker"), member("watch", "worker")],
        "channels": [task_channel("leader", &["b"]), message_channel("b", &["leader"])],
        "observers": [{"agent_id": "watch", "subjects": ["b"], "event_types": ["task_completed"],
                       "payload_scope": "status", "wake_policy": "on_event", "capabilities": []}],
    });
    let core = core_with_spec("t4", spec);
    let leader = scripted(
        "leader",
        &json!([
            ["call", "assign_task", {"assignee": "b", "description": "secret work"}],
            ["call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}],
            ["wait"],
            ["call", "signal_done", {}],
            ["end"],
        ]),
        barriers(),
    );
    let b = scripted(
        "b",
        &json!([
            ["call", "complete_task", {"task_id": "$inbox0.payload.task_id", "result_refs": ["private/out.txt"], "summary": "PRIVATE DETAILS"}],
            ["end"],
        ]),
        barriers(),
    );
    let watch = scripted("watch", &json!([["inbox"], ["end"]]), barriers());
    let h = harness_with(core.clone(), vec![("leader", leader), ("b", b), ("watch", watch.clone())]);
    h.runtime.start();
    h.runtime.user_message("do the secret work", false).unwrap();
    assert!(h.runtime.settle(10));

    let observed = watch.observed_inbox.lock().unwrap().clone();
    assert!(!observed.is_empty(), "on_event observer must receive matching events");
    for item in &observed {
        assert_eq!(item.get("kind").and_then(|v| v.as_str()), Some("task_completed"));
    }
    let payload = observed[0].get("payload").cloned().unwrap_or(Json::Null);
    assert_eq!(payload.get("status").and_then(|v| v.as_str()), Some("SUCCEEDED"));
    assert!(!payload.to_string().contains("PRIVATE DETAILS"));
    assert!(!payload.to_string().contains("private/out.txt"));

    let send = submit(&core, "w1", "watch", "send_message", json!({"target": "b", "text": "hi"}));
    assert!(!send.ok && send.error.unwrap_or_default().contains("not allowed to message"));
    let patch = submit(
        &core,
        "w2",
        "watch",
        "apply_topology_patch",
        json!({"operations": [{"op": "remove_agent", "agent_id": "b"}], "base_revision": 1}),
    );
    assert!(!patch.ok && patch.error.unwrap_or_default().contains("Leader"));
    h.runtime.close();
}

#[test]
fn t5_shared_space_permissions_and_discovery() {
    isolated_state_home("t5");
    let spec = json!({
        "leader_id": "leader",
        "agents": [member("leader", "leader"), member("b", "worker"), member("c", "worker"), member("d", "worker")],
        "channels": [task_channel("leader", &["b", "c"])],
        "shared_spaces": [{"id": "main", "readers": ["leader", "b", "c"], "writers": ["leader", "b"]}],
    });
    let core = core_with_spec("t5", spec);
    let leader = scripted(
        "leader",
        &json!([
            ["call", "assign_task", {"assignee": "b", "description": "publish findings"}],
            // c depends on b: the dependency both models the scenario and makes
            // the read deterministic (core only starts a task once its deps passed)
            ["call", "assign_task", {"assignee": "c", "description": "use findings", "dependencies": ["$r0.result.task_id"]}],
            ["call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id", "$r1.result.task_id"]}],
            ["wait"],
            ["call", "signal_done", {}],
            ["end"],
        ]),
        barriers(),
    );
    let b = scripted(
        "b",
        &json!([
            ["call", "publish_shared", {"space_id": "main", "kind": "finding", "content": "the cache is cold", "ref": "artifacts/trace-1.bin"}],
            ["call", "complete_task", {"task_id": "$inbox0.payload.task_id"}],
            ["end"],
        ]),
        barriers(),
    );
    let c = scripted(
        "c",
        &json!([
            ["inbox"],
            ["call", "read_shared", {"space_id": "main"}],
            ["call", "list_shared", {}],
            ["call", "complete_task", {"task_id": "$inbox0.payload.task_id"}],
            ["end"],
        ]),
        barriers(),
    );
    let d = scripted("d", &json!([]), barriers());
    let h = harness_with(core.clone(), vec![("leader", leader), ("b", b), ("c", c.clone()), ("d", d)]);
    h.runtime.start();
    h.runtime.user_message("share and reuse findings", false).unwrap();
    assert!(h.runtime.settle(10));

    let read = c
        .results
        .lock()
        .unwrap()
        .iter()
        .find(|r| r.get("kind").and_then(|v| v.as_str()) == Some("read_shared"))
        .cloned()
        .expect("read_shared result");
    assert_eq!(read.get("ok").and_then(|v| v.as_bool()), Some(true));
    let entry = read.pointer("/result/entries/0").cloned().unwrap_or(Json::Null);
    assert_eq!(entry.get("content").and_then(|v| v.as_str()), Some("the cache is cold"));
    assert_eq!(entry.get("ref").and_then(|v| v.as_str()), Some("artifacts/trace-1.bin"));

    let denied_write = submit(&core, "d1", "d", "publish_shared", json!({"space_id": "main", "content": "nope"}));
    assert!(!denied_write.ok && denied_write.error.unwrap_or_default().contains("write access"));
    let denied_read = submit(&core, "d2", "d", "read_shared", json!({"space_id": "main"}));
    assert!(!denied_read.ok && denied_read.error.unwrap_or_default().contains("read access"));
    let listed = submit(&core, "d3", "d", "list_shared", json!({}));
    assert!(listed.ok);
    assert_eq!(listed.result.get("spaces").cloned().unwrap_or(Json::Null), json!([]));
    h.runtime.close();
}

#[test]
fn p2_cancel_run_stops_a_slow_member_turn() {
    isolated_state_home("cancel");
    let core = core_with_spec("p2", json!({"leader_id": "leader", "agents": [member("leader", "leader")]}));
    let leader = scripted("leader", &json!([["sleep", 30], ["end"]]), barriers());
    let h = harness_with(core.clone(), vec![("leader", leader)]);
    h.runtime.start();
    h.runtime.user_message("long work", false).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let state = h.runtime.state().unwrap();
    let run = runs(&state).into_iter().find(|r| r.agent_id == "leader").expect("run");
    assert_eq!(run.status, TurnStatus::Running);
    let receipt = submit(&core, "cx1", "user", "cancel_run", json!({"run_id": run.run_id}));
    assert!(receipt.ok);
    assert!(h.runtime.settle(10));
    let after = h.runtime.state().unwrap();
    let run = runs(&after).into_iter().find(|r| r.run_id == run.run_id).unwrap();
    assert_eq!(run.status, TurnStatus::Cancelled);
    h.runtime.close();
}

#[test]
fn paused_session_still_cancels_an_active_member() {
    let core = core_with_spec("paused-cancel", json!({"leader_id": "leader", "agents": [member("leader", "leader")]}));
    let leader = scripted("leader", &json!([["sleep", 30], ["end"]]), barriers());
    let h = harness_with(core.clone(), vec![("leader", leader.clone())]);
    h.runtime.start();
    h.runtime.user_message("long work", false).unwrap();
    assert!(wait_for(|| leader.last_view.lock().unwrap().is_some(), 2000));
    let run = runs(&core.state().unwrap()).into_iter().find(|r| r.agent_id == "leader").unwrap();
    assert!(submit(&core, "pause", "user", "pause_session", json!({})).ok);
    assert!(submit(&core, "cancel", "user", "cancel_run", json!({"run_id": run.run_id})).ok);

    let stopped = wait_for(|| leader.is_cancelled(&run.run_id), 2000);
    let settled = stopped && h.runtime.settle(2);
    h.runtime.close();
    assert!(stopped, "pausing dispatch must not disable cancellation of an executing member");
    assert!(settled);
    let state = core.state().unwrap();
    assert_eq!(state["session"]["status"], "PAUSED");
    assert_eq!(runs(&state).into_iter().find(|r| r.run_id == run.run_id).unwrap().status, TurnStatus::Cancelled);
}

#[test]
fn p2_pause_then_resume_by_user_input() {
    isolated_state_home("pause");
    let core = core_with_spec("p2b", json!({"leader_id": "leader", "agents": [member("leader", "leader")]}));
    let leader = scripted("leader", &json!([["call", "signal_done", {"summary": "s"}], ["end"]]), barriers());
    let h = harness_with(core.clone(), vec![("leader", leader)]);
    h.runtime.start();
    let receipt = submit(&core, "ps1", "user", "pause_session", json!({}));
    assert!(receipt.ok);
    assert_eq!(
        h.runtime.state().unwrap().get("session").and_then(|s| s.get("status")).and_then(|v| v.as_str()),
        Some("PAUSED")
    );
    h.runtime.user_message("resume please", false).unwrap();
    assert!(h.runtime.settle(10));
    let state = h.runtime.state().unwrap();
    assert_eq!(state.get("session").and_then(|s| s.get("status")).and_then(|v| v.as_str()), Some("ACTIVE"));
    assert_eq!(state.get("session").and_then(|s| s.get("goal_state")).and_then(|v| v.as_str()), Some("done"));
    h.runtime.close();
}

#[test]
fn full_auto_toggle_reaches_the_approval_gate() {
    isolated_state_home("fullauto");
    let core = core_with_spec("fa", json!({"leader_id": "leader", "agents": [member("leader", "leader")]}));
    let gate = teamagents_engine::gateway::ApprovalGate::new(
        core.clone(),
        teamagents_engine::gateway::PermissionPolicy::default(),
    );
    // network shell is outside the pre-authorized scope: it must pause
    let (decision, approval) =
        gate.check("leader", "run-fa", "shell", &json!({"command": "curl x", "network": true}), "c1").unwrap();
    assert!(!decision.allow);
    assert_eq!(approval.map(|a| a.status), Some(teamagents_core::models::ApprovalStatus::Pending));

    // the user flips the session to full auto; the gate follows without restart
    let receipt = submit(&core, "mode-1", "user", "set_permission_mode", json!({"mode": "full_auto"}));
    assert!(receipt.ok);
    let (decision, approval) =
        gate.check("leader", "run-fa", "shell", &json!({"command": "curl x", "network": true}), "c2").unwrap();
    assert!(decision.allow, "full auto allows the same call");
    assert!(approval.is_none());
}
