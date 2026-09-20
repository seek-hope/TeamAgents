//! T17: cold recovery through the production session/runtime and a durable
//! protocol fixture. No model or user Codex state is accessed.

mod support;

use serde_json::{json, Value as Json};
use std::path::Path;
use std::sync::Arc;
use support::*;
use teamagents_core::models::TurnRun;
use teamagents_engine::session::{open_session, OpenOptions, OpenedSession};

fn fixture(env: &mut TestEnv) {
    // The stable shell reads app-server in the member's workspace. Avoid
    // executing a freshly written file under parallel process creation.
    let bin = env.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink("/bin/sh", bin.join("codex")).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    env.set("PATH", format!("{}:{}", bin.display(), path.to_string_lossy()));
    env.set("TA_RECOVERY_LOG", env.join("requests.jsonl"));
    env.set("TA_RECOVERY_HISTORY", env.join("history.json"));
    std::fs::create_dir_all(env.join("project")).unwrap();
    std::fs::write(
        env.join("project/app-server"),
        r#"exec python3 -u -c '
import json, os, sys
for line in sys.stdin:
    request = json.loads(line)
    with open(os.environ["TA_RECOVERY_LOG"], "a") as log:
        log.write(json.dumps(request) + "\n")
    if "id" not in request:
        continue
    method = request["method"]
    if method == "initialize":
        result = {"userAgent":"recovery-fixture"}
    elif method == "thread/read":
        with open(os.environ["TA_RECOVERY_HISTORY"]) as history:
            result = json.load(history)
        if "error" in result:
            print(json.dumps({"id":request["id"],"error":result["error"]}), flush=True)
            continue
    elif (os.environ.get("TA_RECOVERY_CRASH_SETUP") or os.environ.get("TA_RECOVERY_DISCONNECT")) and method == "thread/start":
        result = {"thread":{"id":"saved-thread"}}
    elif (os.environ.get("TA_RECOVERY_CRASH_SETUP") or os.environ.get("TA_RECOVERY_DISCONNECT")) and method == "turn/start":
        with open("effect.txt", "a") as effect:
            effect.write("executed once\n")
        turn = {"id":"saved-turn","status":"completed","items":[
            {"type":"agentMessage","text":"persisted result before the client crashed"}]}
        with open(os.environ["TA_RECOVERY_HISTORY"], "w") as history:
            json.dump({"thread":{"id":"saved-thread","turns":[turn]}}, history)
        result = {"turn":{"id":"saved-turn","status":"inProgress"}}
        if os.environ.get("TA_RECOVERY_DISCONNECT") == "before":
            sys.exit(0)
        if os.environ.get("TA_RECOVERY_DISCONNECT") == "missing-id":
            result = {"turn":{"status":"inProgress"}}
    else:
        print(json.dumps({"id":request["id"],"error":{"code":-32601,"message":"recovery must only read"}}), flush=True)
        continue
    print(json.dumps({"id":request["id"],"result":result}), flush=True)
    if method == "turn/start" and os.environ.get("TA_RECOVERY_DISCONNECT") == "after":
        sys.exit(0)
'
"#,
    )
    .unwrap();
}

fn open(root: &Path, id: &str) -> Arc<OpenedSession> {
    open_session(OpenOptions {
        cwd: Some(root.join("project")),
        session_id: Some(id.into()),
        catalog: Some(serde_json::from_value(test_catalog()).unwrap()),
        initial_spec: Some(json!({
            "leader_id":"leader",
            "agents":[member("leader","leader"),
                {"id":"cx","name":"Codex","role":"worker","runtime_kind":"codex","model_profile":"m"}],
            "channels":[task_channel("leader",&["cx"]),message_channel("leader",&["cx"])]
        })),
        ..Default::default()
    })
    .unwrap()
}

fn parked(session: &OpenedSession, external: Option<&str>, confirm: bool) -> TurnRun {
    let assigned = submit(
        &session.core,
        "assign",
        "leader",
        "assign_task",
        json!({"assignee":"cx","description":"repair the parser","acceptance":"all parser tests pass"}),
    );
    assert!(assigned.ok, "{assigned:?}");
    let state = session.core.state_brief().unwrap();
    let run = state["runs"].as_array().unwrap().iter().find(|r| r["agent_id"] == "cx").unwrap();
    let run = session.core.call_in_session("begin_run", json!({"run_id":run["run_id"]})).unwrap()["run"].clone();
    let mut run: TurnRun = serde_json::from_value(run).unwrap();
    if let Some(external) = external {
        session.core.call_in_session("set_codex_thread", json!({"agent_id":"cx","thread_id":"saved-thread"})).unwrap();
        session
            .core
            .call_in_session("set_run_external_turn", json!({"run_id":run.run_id,"external_turn_id":external}))
            .unwrap();
        run.external_turn_id = Some(external.into());
    }
    if confirm {
        session
            .core
            .call_in_session("confirm_delivery_ids", json!({"run_id":run.run_id,"delivery_ids":run.input_delivery_ids}))
            .unwrap();
    }
    run
}

fn history(root: &Path, turns: Json) {
    std::fs::write(root.join("history.json"), json!({"thread":{"id":"saved-thread","turns":turns}}).to_string())
        .unwrap();
}

fn methods(root: &Path) -> Vec<String> {
    std::fs::read_to_string(root.join("requests.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str::<Json>(line).unwrap()["method"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn cold_codex_recovery_restores_the_matching_result_and_is_idempotent() {
    let mut env = isolated_state_home("codex-cold-result");
    fixture(&mut env);
    let session = open(&env, "cold-result");
    let run = parked(&session, Some("saved-turn"), true);
    let unsent = submit(
        &session.core,
        "unsent-followup",
        "leader",
        "send_message",
        json!({"target":"cx","text":"follow-up never accepted by the external turn"}),
    );
    assert!(unsent.ok);
    session.close();
    drop(session);
    history(
        &env,
        json!([
            {"id":"saved-turn","status":"completed","items":[
                {"type":"reasoning","text":"PRIVATE_REASONING_MUST_NOT_BE_PUBLISHED"},
                {"type":"commandExecution","aggregatedOutput":"PRIVATE_TOOL_OUTPUT"},
                {"type":"agentMessage","text":"parser repaired; 12 tests passed"}
            ]},
            {"id":"unrelated-later-turn","status":"failed","items":[]}
        ]),
    );
    let restored = open(&env, "cold-result");
    restored.runtime.reconcile();
    let first = restored.core.state().unwrap();
    restored.runtime.reconcile();
    let second = restored.core.state().unwrap();
    let remaining = restored.core.call_in_session("agent_view", json!({"agent_id":"cx"})).unwrap();
    restored.close();
    let recovered = first["runs"].as_array().unwrap().iter().find(|r| r["run_id"] == run.run_id).unwrap();
    assert_eq!(recovered["status"], "COMPLETED", "{first}");
    assert_eq!(first["tasks"][0]["status"], "SUCCEEDED", "{first}");
    let completed: Vec<_> =
        first["events"].as_array().unwrap().iter().filter(|e| e["kind"] == "task_completed").collect();
    assert_eq!(completed.len(), 1, "{first}");
    assert_eq!(completed[0]["payload"]["summary"], "parser repaired; 12 tests passed");
    assert_eq!(first["events"], second["events"], "reconcile cannot repeat completion");
    assert!(!first.to_string().contains("PRIVATE_"));
    assert_eq!(remaining["inbox_delta"].as_array().unwrap().len(), 1);
    assert!(remaining["inbox_delta"][0]["payload"].to_string().contains("follow-up never accepted"));
    assert_eq!(methods(&env), ["initialize", "thread/read"], "recovery must not submit or resume a turn");
}

#[test]
fn cold_codex_recovery_preserves_failures_and_rejects_unverifiable_history() {
    let mut env = isolated_state_home("codex-cold-status");
    fixture(&mut env);
    for (index, turns, expected, detail) in [
        (
            0,
            json!([{"id":"saved-turn","status":"failed","error":{"message":"parser check failed"},"items":[]}]),
            "FAILED",
            Some("parser check failed"),
        ),
        (1, json!([{"id":"saved-turn","status":"interrupted","items":[]}]), "CANCELLED", None),
        (2, json!([{"id":"different-turn","status":"completed","items":[]}]), "OUTCOME_UNKNOWN", None),
        (3, json!([{"id":"saved-turn","status":"inProgress","items":[]}]), "OUTCOME_UNKNOWN", None),
        (4, json!([]), "OUTCOME_UNKNOWN", None),
        (5, json!([{"id":"saved-turn","status":"unexpected","items":[]}]), "OUTCOME_UNKNOWN", None),
    ] {
        let id = format!("cold-status-{index}");
        let session = open(&env, &id);
        let run = parked(&session, Some("saved-turn"), true);
        session.close();
        drop(session);
        history(&env, turns);
        let restored = open(&env, &id);
        restored.runtime.reconcile();
        let state = restored.core.state().unwrap();
        restored.close();
        assert_eq!(
            state["runs"].as_array().unwrap().iter().find(|r| r["run_id"] == run.run_id).unwrap()["status"],
            expected,
            "{state}"
        );
        if let Some(detail) = detail {
            assert!(
                state["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|e| e["kind"] == "run_failed"
                        && e["payload"]["error"].as_str().is_some_and(|s| s.contains(detail))),
                "{state}"
            );
        }
        assert!(!state["events"].as_array().unwrap().iter().any(|e| e["kind"] == "task_completed"));
    }
    assert!(methods(&env).iter().all(|m| matches!(m.as_str(), "initialize" | "thread/read")));
}

#[test]
fn missing_codex_turn_id_cannot_blindly_requeue_accepted_work() {
    for queued in [false, true] {
        let mut env = isolated_state_home("codex-missing-turn-id");
        fixture(&mut env);
        let session = open(&env, "missing-turn");
        let run = parked(&session, None, false);
        if queued {
            session.core.call_in_session("requeue_run", json!({"run_id":run.run_id})).unwrap();
        }
        session.close();
        drop(session);
        let restored = open(&env, "missing-turn");
        restored.runtime.reconcile();
        let state = restored.core.state().unwrap();
        restored.close();
        let worker_runs: Vec<_> = state["runs"].as_array().unwrap().iter().filter(|r| r["agent_id"] == "cx").collect();
        assert_eq!(worker_runs.len(), 1, "uncertain input must not create a replacement run: {state}");
        assert_eq!(worker_runs[0]["run_id"], run.run_id);
        assert_eq!(worker_runs[0]["status"], "OUTCOME_UNKNOWN", "{state}");
        assert!(methods(&env).is_empty(), "without a saved ID no external turn can be identified");
    }
}

#[test]
fn cold_codex_recovery_expires_stale_approvals_and_checks_thread_identity() {
    let mut env = isolated_state_home("codex-cold-approval");
    fixture(&mut env);
    for (index, response, expected) in [
        (
            0,
            json!({"thread":{"id":"wrong-thread","turns":[{"id":"saved-turn","status":"completed","items":[]}]}}),
            "OUTCOME_UNKNOWN",
        ),
        (
            1,
            json!({"thread":{"id":"saved-thread","turns":[{"id":"saved-turn","status":"completed","items":[]},
            {"id":"saved-turn","status":"failed","items":[]}]}}),
            "OUTCOME_UNKNOWN",
        ),
        (
            2,
            json!({"thread":{"id":"saved-thread","turns":[{"id":"saved-turn","status":"completed"}]}}),
            "OUTCOME_UNKNOWN",
        ),
        (3, json!({"error":{"code":-32000,"message":"history unavailable"}}), "OUTCOME_UNKNOWN"),
        (
            4,
            json!({"thread":{"id":"saved-thread","turns":[{"id":"saved-turn","status":"completed","items":[
                {"type":"agentMessage","text":"recorded completion"}
            ]}]}}),
            "COMPLETED",
        ),
    ] {
        let id = format!("cold-approval-{index}");
        let session = open(&env, &id);
        let run = parked(&session, Some("saved-turn"), true);
        session
            .core
            .call_in_session(
                "insert_approval",
                json!({"approval":{
                    "approval_id":"stale-approval","session_id":id,"agent_id":"cx","run_id":run.run_id,
                    "tool_call_id":"external-command","operation_hash":"hash","requested_scope":{},
                    "policy_revision":1
                }}),
            )
            .unwrap();
        session
            .core
            .call_in_session("set_run_status", json!({"run_id":run.run_id,"status":"WAITING_APPROVAL"}))
            .unwrap();
        session.close();
        drop(session);
        std::fs::write(env.join("history.json"), response.to_string()).unwrap();
        let restored = open(&env, &id);
        restored.runtime.reconcile();
        let state = restored.core.state().unwrap();
        let approval = restored.core.call_in_session("get_approval", json!({"approval_id":"stale-approval"})).unwrap();
        restored.close();
        assert_eq!(
            state["runs"].as_array().unwrap().iter().find(|r| r["run_id"] == run.run_id).unwrap()["status"],
            expected,
            "{state}"
        );
        assert_eq!(approval["approval"]["status"], "EXPIRED", "{approval}");
        assert!(state["pending_approvals"].as_array().unwrap().is_empty());
    }
    assert!(methods(&env).iter().all(|m| matches!(m.as_str(), "initialize" | "thread/read")));
}

#[test]
fn cold_codex_recovery_reuses_the_committed_completion_request() {
    use teamagents_engine::gateway::{ApprovalGate, PermissionPolicy, ToolGateway};
    let mut env = isolated_state_home("codex-committed-completion");
    fixture(&mut env);
    let session = open(&env, "committed-completion");
    let run = parked(&session, Some("saved-turn"), true);
    let gateway = ToolGateway::new(
        session.core.clone(),
        "cx",
        &run.run_id,
        ApprovalGate::new(session.core.clone(), PermissionPolicy::default()),
        None,
    );
    let receipt = gateway.call(
        "complete_task",
        &json!({"task_id":run.task_id,"summary":"streamed commentaryfinal answer","result_refs":[]}),
        "saved-turn:complete",
    );
    assert!(receipt.ok);
    session.close();
    drop(gateway);
    drop(session);
    // Codex history stores distinct messages, unlike the joined live deltas.
    history(
        &env,
        json!([{"id":"saved-turn","status":"completed","items":[
            {"type":"agentMessage","text":"streamed commentary"},
            {"type":"agentMessage","text":"final answer"}
        ]}]),
    );
    let restored = open(&env, "committed-completion");
    restored.runtime.reconcile();
    let state = restored.core.state().unwrap();
    restored.close();
    assert_eq!(state["tasks"][0]["status"], "SUCCEEDED", "{state}");
    let events: Vec<_> = state["events"].as_array().unwrap().iter().filter(|e| e["kind"] == "task_completed").collect();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0]["payload"]["summary"], "streamed commentaryfinal answer",
        "the committed request is immutable"
    );
}

#[test]
fn killed_codex_client_recovers_persisted_completion_without_reexecuting() {
    use teamagents_engine::core_client::CoreClient;
    use teamagents_engine::gateway::{ApprovalGate, PermissionPolicy, ToolGateway};
    use teamagents_engine::sessions::session_paths;

    if let Some(root) = std::env::var_os("TA_RECOVERY_CRASH_CHILD") {
        let session = open(Path::new(&root), "killed-codex");
        let run = parked(&session, None, false);
        let view = session.core.call_in_session("agent_view", json!({"agent_id":"cx"})).unwrap();
        let runner = session.runtime.runner("cx").unwrap();
        let gateway = ToolGateway::new(
            session.core.clone(),
            "cx",
            &run.run_id,
            ApprovalGate::new(session.core.clone(), PermissionPolicy::default()),
            None,
        );
        let outcome = runner.start_or_resume(&run, &view, &gateway, &Json::Null);
        panic!("parent should kill this client before notification: {outcome:?}");
    }
    let mut env = isolated_state_home("codex-killed-client");
    fixture(&mut env);
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "killed_codex_client_recovers_persisted_completion_without_reexecuting", "--nocapture"])
        .env("TA_RECOVERY_CRASH_CHILD", env.as_os_str())
        .env("TA_RECOVERY_CRASH_SETUP", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(env.join("child.log")).unwrap())
        .spawn()
        .unwrap();
    let db = session_paths("killed-codex").db;
    let ready = wait_for(
        || {
            if !db.is_file() {
                return false;
            }
            let Ok(core) = CoreClient::open(db.to_str().unwrap(), "killed-codex") else { return false };
            let Ok(state) = core.state_brief() else { return false };
            state["runs"].as_array().is_some_and(|runs| {
                runs.iter()
                    .any(|r| r["agent_id"] == "cx" && r["external_turn_id"] == "saved-turn" && r["status"] == "RUNNING")
            })
        },
        10_000,
    );
    let _ = child.kill();
    let exit = child.wait().unwrap();
    assert!(
        ready,
        "child never reached the persisted external turn boundary: {}",
        std::fs::read_to_string(env.join("child.log")).unwrap()
    );
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(exit.signal(), Some(9), "the recovery path requires an actual SIGKILL");
    }
    let restored = open(&env, "killed-codex");
    restored.runtime.reconcile();
    let state = restored.core.state().unwrap();
    restored.close();
    assert_eq!(state["tasks"][0]["status"], "SUCCEEDED", "{state}");
    assert_eq!(std::fs::read_to_string(env.join("project/effect.txt")).unwrap(), "executed once\n");
    let calls = methods(&env);
    assert_eq!(calls.iter().filter(|m| *m == "turn/start").count(), 1);
    assert_eq!(calls.iter().filter(|m| *m == "thread/read").count(), 1);
    assert!(!calls.iter().any(|m| m == "thread/resume"));
}

#[test]
fn disconnected_codex_submission_never_schedules_automatic_reexecution() {
    let mut env = isolated_state_home("codex-disconnect");
    fixture(&mut env);
    for phase in ["before", "after", "missing-id"] {
        env.set("TA_RECOVERY_DISCONNECT", phase);
        let session = open(&env, &format!("disconnect-{phase}"));
        session.runtime.add_runner("leader", scripted("leader", &json!([["end"]]), barriers()));
        let assigned = submit(
            &session.core,
            "assign",
            "leader",
            "assign_task",
            json!({"assignee":"cx","description":"perform the operation once","acceptance":"one side effect"}),
        );
        assert!(assigned.ok, "{assigned:?}");
        session.runtime.start();
        assert!(session.runtime.settle(10));
        let state = session.core.state().unwrap();
        session.close();
        let runs: Vec<_> = state["runs"].as_array().unwrap().iter().filter(|r| r["agent_id"] == "cx").collect();
        assert_eq!(runs.len(), 1, "{state}");
        assert_eq!(runs[0]["status"], "OUTCOME_UNKNOWN", "{state}");
        assert_eq!(state["tasks"][0]["status"], "BLOCKED");
    }
    assert_eq!(methods(&env).iter().filter(|m| *m == "turn/start").count(), 3, "one attempt per independent scenario");
    assert_eq!(std::fs::read_to_string(env.join("project/effect.txt")).unwrap(), "executed once\n".repeat(3));
}
