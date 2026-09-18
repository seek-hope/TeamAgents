//! Codex backend contracts from the review: reconcile reads the thread history
//! with `thread/read` (F-5) and an approval that timed out is voided in the
//! core instead of lingering as PENDING (F-8).

mod support;
use support::isolated_state_home as env_guard;

use serde_json::{json, Value as Json};
use std::sync::Arc;
use support::{core_with_spec, member, wait_for};
use teamagents_core::models::{TurnRun, TurnStatus};
use teamagents_engine::codex::{AppServerOptions, CodexAppServer, CodexOptions, CodexRunner};
use teamagents_engine::core_client::CoreClient;
use teamagents_engine::gateway::{ApprovalGate, PermissionPolicy, ToolGateway};
use teamagents_engine::runtime::{AgentRunner, Notify};

fn codex_agent() -> Json {
    json!({"id": "cx", "name": "C", "role": "worker", "runtime_kind": "codex", "model_profile": "m"})
}

fn run_for(session: &str, run_id: &str, external_turn_id: Option<&str>) -> TurnRun {
    serde_json::from_value(json!({
        "run_id": run_id, "session_id": session, "task_id": null, "goal_id": null, "agent_id": "cx",
        "config_revision": 1, "topology_revision": 1, "status": "QUEUED", "input_delivery_ids": [],
        "context_ref": null, "external_turn_id": external_turn_id, "cancel_requested": false,
        "waiting_on": [], "created_at": 0, "updated_at": 0,
    }))
    .unwrap()
}

fn view() -> Json {
    json!({"agent_id": "cx", "assignment": [], "inbox_delta": [], "permitted_shared_delta": [],
           "relevant_topology": {"revision": 1}, "delivery_ids": [], "batch_no": 0})
}

fn gateway(core: &Arc<CoreClient>, run_id: &str) -> Arc<ToolGateway> {
    ToolGateway::new(
        core.clone(),
        "cx",
        run_id,
        ApprovalGate::new(core.clone(), PermissionPolicy::default()),
        Some(Arc::new(|tool: &str, _args: &Json| Err(format!("no executor for {tool}")))),
    )
}

fn runner_with(core: &Arc<CoreClient>, session: &str, bin: &str, extra_env: Vec<(&str, &str)>) -> Arc<CodexRunner> {
    CodexRunner::new(
        CodexOptions {
            agent_id: "cx".into(),
            session_id: session.into(),
            instructions: extra_env
                .iter()
                .find(|(k, _)| *k == "FAKE_INSTRUCTIONS")
                .map(|(_, v)| v.to_string())
                .unwrap_or_default(),
            workdir: std::env::temp_dir(),
            sandbox: "workspace-write".into(),
            approval_policy: "on-request".into(),
            effort: None,
            model: extra_env.iter().find(|(k, _)| *k == "FAKE_MODEL").map(|(_, v)| v.to_string()),
            codex_bin: Some(bin.into()),
            codex_home: None,
            config_overrides: extra_env
                .iter()
                .find(|(k, _)| *k == "FAKE_PROVIDER")
                .map(|(_, v)| vec![("model_provider".into(), json!(v))])
                .unwrap_or_default(),
            env: extra_env.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        },
        core.clone(),
        ApprovalGate::new(core.clone(), PermissionPolicy::default()),
        Notify::new(core.clone()),
    )
}

/// A `codex app-server` stand-in that serves `thread/read` and logs every
/// method it was asked for. `FAKE_TURNS` picks the recorded turn history.
const FAKE_HISTORY_SERVER: &str = r#"#!/usr/bin/env python3
import json, os, sys

mode = os.environ.get("FAKE_TURNS", "completed")
log = os.environ.get("FAKE_METHOD_LOG")
turns = {
    "completed": [{"id": "t1", "status": "completed"}],
    "inprogress": [{"id": "t1", "status": "inProgress"}],
    "empty": [],
}.get(mode, [{"id": "t1", "status": "completed"}])

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    message = json.loads(line)
    if os.environ.get("FAKE_REQUEST_LOG"):
        with open(os.environ["FAKE_REQUEST_LOG"], "a") as fh:
            fh.write(json.dumps(message) + "\n")
    method = message.get("method")
    if method is None:
        continue
    if log:
        with open(log, "a") as fh:
            fh.write(method + "\n")
    result = {}
    if method == "initialize":
        result = {"userAgent": "fake-history/0.1"}
    elif method in ("thread/start", "thread/resume"):
        result = {"thread": {"id": "thr-1"}}
    elif method == "turn/start":
        result = {"turn": {"id": "turn-1", "status": "inProgress"}}
        print(json.dumps({"method": "item/completed", "params": {
            "threadId": "thr-1", "turnId": "turn-1",
            "item": {"type": "agentMessage", "text": "history done"}}}), flush=True)
        print(json.dumps({"method": "turn/completed", "params": {
            "threadId": "thr-1", "turn": {"id": "turn-1", "status": "completed"}}}), flush=True)
    elif method == "thread/read":
        result = {"thread": {"id": "thr-1", "turns": turns}}
    print(json.dumps({"id": message.get("id"), "result": result}), flush=True)
"#;

fn fake_history_server(dir: &std::path::Path) -> String {
    let path = dir.join("fake_history_server.py");
    std::fs::write(&path, FAKE_HISTORY_SERVER).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path.to_string_lossy().into_owned()
}

#[test]
fn model_switch_resumes_codex_thread_with_selected_provider_and_model() {
    let _env = env_guard("codex-model-switch");
    let dir = std::env::temp_dir().join(format!("ta-codex-model-switch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let bin = fake_history_server(&dir);
    let log = dir.join("requests.jsonl");
    let log_str = log.to_string_lossy();
    let core =
        core_with_spec("cx-model", json!({"leader_id":"leader", "agents":[member("leader","leader"),codex_agent()]}));
    for model in ["old-model", "new-model"] {
        let runner = runner_with(
            &core,
            "cx-model",
            &bin,
            vec![("FAKE_MODEL", model), ("FAKE_PROVIDER", model), ("FAKE_REQUEST_LOG", &log_str)],
        );
        let outcome = runner.start_or_resume(
            &run_for("cx-model", model, None),
            &view(),
            &gateway(&core, model),
            &json!({"reason":"new_input"}),
        );
        assert_eq!(outcome.status, TurnStatus::Completed);
        runner.close();
    }
    let requests: Vec<Json> =
        std::fs::read_to_string(&log).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let resume = requests.iter().find(|r| r["method"] == "thread/resume").expect("must load the persisted thread");
    assert_eq!(resume["params"]["threadId"], "thr-1");
    assert_eq!(resume["params"]["model"], "new-model");
    assert_eq!(resume["params"]["modelProvider"], "new-model");
    let models: Vec<_> = requests
        .iter()
        .filter(|r| r["method"] == "turn/start")
        .map(|r| r["params"]["model"].as_str().unwrap())
        .collect();
    assert_eq!(models, ["old-model", "new-model"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn worker_environment_is_developer_instructions_on_codex_start_and_resume() {
    let _env = env_guard("codex-worker-prompt");
    let dir = std::env::temp_dir().join(format!("ta-codex-worker-prompt-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let bin = fake_history_server(&dir);
    let log = dir.join("requests.jsonl");
    let log_str = log.to_string_lossy();
    let core =
        core_with_spec("cx-prompt", json!({"leader_id":"leader", "agents":[member("leader","leader"),codex_agent()]}));
    let mut task_view = view();
    task_view["assignment"] =
        json!([{"task_id":"t1", "description":"Check the parser", "acceptance":"Show test evidence"}]);
    for (index, instructions) in ["", "Inspect boundary conditions."].into_iter().enumerate() {
        let runner = runner_with(
            &core,
            "cx-prompt",
            &bin,
            vec![("FAKE_INSTRUCTIONS", instructions), ("FAKE_REQUEST_LOG", &log_str)],
        );
        let id = format!("run-prompt-{index}");
        let outcome = runner.start_or_resume(
            &run_for("cx-prompt", &id, None),
            &task_view,
            &gateway(&core, &id),
            &json!({"reason":"new_input"}),
        );
        assert_eq!(outcome.status, TurnStatus::Completed);
        runner.close();
    }
    let requests: Vec<Json> =
        std::fs::read_to_string(&log).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    for method in ["thread/start", "thread/resume"] {
        let index = requests.iter().position(|r| r["method"] == method).unwrap();
        let params = &requests[index]["params"];
        let environment = params["developerInstructions"].as_str().unwrap();
        assert!(environment.starts_with("<teamagents_worker>"));
        assert!(environment.contains("not provided to this execution backend"));
        assert!(params.get("baseInstructions").is_none(), "preserve the native Codex system prompt");
        assert!(!environment.contains("Check the parser"));
        if method == "thread/resume" {
            assert!(
                environment.find("</teamagents_worker>").unwrap()
                    < environment.find("Inspect boundary conditions.").unwrap()
            );
        }
        let turn = requests[index + 1..].iter().find(|r| r["method"] == "turn/start").unwrap();
        assert!(turn["params"]["input"][0]["text"].as_str().unwrap().contains("Check the parser"));
        assert!(!turn["params"]["input"][0]["text"].as_str().unwrap().contains("<teamagents_worker>"));
    }
    std::fs::remove_dir_all(dir).unwrap();
}

fn python_available() -> bool {
    std::process::Command::new("python3").arg("--version").output().map(|out| out.status.success()).unwrap_or(false)
}

/// F-5: reconcile uses `thread/read` (the app-server has no `thread/status`
/// method) and maps the last turn.
#[test]
fn reconcile_reads_the_thread_history() {
    if !python_available() {
        eprintln!("skip: python3 not available for the fake app-server");
        return;
    }
    let _env = env_guard("codex-history");
    let dir = std::env::temp_dir().join(format!("ta-codex-history-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let bin = fake_history_server(&dir);
    let log = dir.join("methods.log");
    let log_str = log.to_string_lossy().into_owned();

    for (mode, session, expected) in [
        ("completed", "cx-hist-1", Some(TurnStatus::Completed)),
        ("inprogress", "cx-hist-2", Some(TurnStatus::OutcomeUnknown)),
        ("empty", "cx-hist-3", None),
    ] {
        let core = core_with_spec(
            session,
            json!({"leader_id": "leader", "agents": [member("leader", "leader"), codex_agent()]}),
        );
        let runner = runner_with(&core, session, &bin, vec![("FAKE_TURNS", mode), ("FAKE_METHOD_LOG", &log_str)]);
        let run = run_for(session, &format!("{session}-run"), None);
        let gw = gateway(&core, &format!("{session}-run"));
        let outcome = runner.start_or_resume(&run, &view(), &gw, &json!({"reason": "new_input"}));
        assert_eq!(outcome.status, TurnStatus::Completed, "{mode}: setup turn completes");

        let parked = run_for(session, &format!("{session}-run"), Some("turn-1"));
        assert_eq!(runner.reconcile(&parked), expected, "{mode}: history decides the status");
        runner.close();
    }
    let methods = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(methods.contains("thread/read"), "reconcile must read the thread: {methods}");
    assert!(!methods.contains("thread/status"), "thread/status does not exist in the app-server schema: {methods}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// F-8: when the approval wait times out the app-server move on, so the core
/// row must go EXPIRED — a later user decision would otherwise be ignored.
#[test]
fn codex_approval_timeout_expires_the_row() {
    let mut env = env_guard("codex-approval-timeout");
    env.set("TEAMAGENTS_CODEX_APPROVAL_WAIT_S", "1");
    let core = core_with_spec(
        "cx-timeout",
        json!({"leader_id": "leader", "agents": [member("leader", "leader"), codex_agent()]}),
    );
    let runner =
        runner_with(&core, "cx-timeout", env!("CARGO_BIN_EXE_fake-codex"), vec![("FAKE_CODEX_MODE", "approval")]);
    let run = run_for("cx-timeout", "cx-timeout-run", None);
    let gw = gateway(&core, "cx-timeout-run");
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let runner = runner.clone();
        let view = view();
        let wake = json!({"reason": "new_input"});
        std::thread::spawn(move || {
            let outcome = runner.start_or_resume(&run, &view, &gw, &wake);
            let _ = tx.send(outcome);
        });
    }
    let requested = wait_for(
        || {
            core.state()
                .ok()
                .and_then(|state| state.get("pending_approvals").and_then(|v| v.as_array()).cloned())
                .map(|pending| !pending.is_empty())
                .unwrap_or(false)
        },
        10_000,
    );
    assert!(requested, "the approval request is recorded");
    let state = core.state().unwrap();
    let approval_id = state
        .get("pending_approvals")
        .and_then(|v| v.as_array())
        .and_then(|pending| pending[0].get("approval_id"))
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

    let outcome = rx
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("a timed-out approval must not wait for the full 600s");
    assert_eq!(outcome.status, TurnStatus::Completed, "the fake server declines and finishes");
    let status = core
        .call_in_session("get_approval", json!({"approval_id": approval_id}))
        .ok()
        .and_then(|reply| reply.pointer("/approval/status").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_default();
    assert_eq!(status, "EXPIRED", "a timed-out approval is void, not silently ignored");
    let pending = core
        .state()
        .ok()
        .and_then(|state| state.get("pending_approvals").and_then(|v| v.as_array()).map(|a| a.len()))
        .unwrap_or(0);
    assert_eq!(pending, 0, "no dead PENDING row is left behind");
    runner.close();
}

/// F-10: closing the app-server kills its whole process group, not only the
/// direct child (shell commands the app-server spawned must not survive).
#[test]
fn closing_the_app_server_kills_its_process_group() {
    if !python_available() {
        eprintln!("skip: python3 not available for the fake app-server");
        return;
    }
    let _env = env_guard("codex-process-group");
    let dir = std::env::temp_dir().join(format!("ta-codex-killpg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("spawner.py");
    std::fs::write(
        &script,
        r#"#!/usr/bin/env python3
import json, os, subprocess, sys

child = subprocess.Popen(["sleep", "30"])
with open(os.environ["FAKE_CHILD_PID"], "w") as fh:
    fh.write(str(child.pid))
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    message = json.loads(line)
    if message.get("method"):
        print(json.dumps({"id": message.get("id"), "result": {}}), flush=True)
"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let pidfile = dir.join("child.pid");
    let server = CodexAppServer::new(
        &dir,
        AppServerOptions {
            codex_bin: Some(script.to_string_lossy().into_owned()),
            env: vec![("FAKE_CHILD_PID".into(), pidfile.to_string_lossy().into_owned())],
            ..Default::default()
        },
    );
    server.start().expect("initialize");
    assert!(wait_for(|| pidfile.is_file(), 5000), "the fake server spawned its child");
    let child_pid: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
    assert!(alive(child_pid), "the child runs before close (control)");

    server.close();
    assert!(
        wait_for(|| !alive(child_pid), 5000),
        "the spawned child of the app-server survives close() (pid {child_pid})"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn alive(pid: i32) -> bool {
    // A zombie is not running: in environments whose PID 1 does not reap
    // orphans (CI containers) a killed child can linger as one.
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { return false };
    let state = stat.rsplit_once(')').map(|(_, rest)| rest.trim_start()).unwrap_or("").chars().next();
    state != Some('Z')
}

/// D-31: a "session" decision answers the app-server with a single-op
/// `accept` — never acceptForSession, whose cache is not bound to one
/// operation_hash — and the core-bound grant auto-accepts the next identical
/// operation (fresh itemId/startedAtMs and all) without a second PENDING row.
#[test]
fn codex_session_grant_auto_accepts_the_identical_operation() {
    let _env = env_guard("codex-session-grant");
    let dir = std::env::temp_dir().join(format!("ta-codex-grant-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("approvals.log");
    let log_str = log.to_string_lossy().into_owned();
    let core = core_with_spec(
        "cx-grant",
        json!({"leader_id": "leader", "agents": [member("leader", "leader"), codex_agent()]}),
    );
    let runner = runner_with(
        &core,
        "cx-grant",
        env!("CARGO_BIN_EXE_fake-codex"),
        vec![("FAKE_CODEX_MODE", "approval-grant"), ("FAKE_APPROVAL_LOG", &log_str)],
    );
    let run = run_for("cx-grant", "cx-grant-run", None);
    let gw = gateway(&core, "cx-grant-run");
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let runner = runner.clone();
        std::thread::spawn(move || {
            let _ = tx.send(runner.start_or_resume(&run, &view(), &gw, &json!({"reason": "new_input"})));
        });
    }
    let requested = wait_for(
        || {
            core.state()
                .ok()
                .and_then(|state| state.get("pending_approvals").and_then(|v| v.as_array()).cloned())
                .map(|pending| !pending.is_empty())
                .unwrap_or(false)
        },
        10_000,
    );
    assert!(requested, "the first approval request is recorded");
    let approval_id =
        core.state().unwrap().pointer("/pending_approvals/0/approval_id").and_then(|v| v.as_str()).unwrap().to_string();
    // decide "session" through the core, as the TUI's DecideApproval effect does
    let action: teamagents_core::models::TeamAction = serde_json::from_value(json!({
        "action_id": "grant-decide-1", "session_id": "cx-grant", "actor_id": "user",
        "kind": "approval_decision",
        "payload": {"approval_id": approval_id, "decision": "session"},
    }))
    .unwrap();
    let receipt = core.submit(&action).expect("decide");
    assert!(receipt.ok, "decision rejected: {receipt:?}");
    assert!(runner.resolve_approval(&approval_id, "session"));

    let outcome = rx
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the grant answers the second request; the turn completes");
    assert_eq!(outcome.status, TurnStatus::Completed);
    let decisions = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(decisions.lines().collect::<Vec<_>>(), ["accept", "accept"], "both replies are single-op accepts");
    let pending = core
        .state()
        .ok()
        .and_then(|state| state.get("pending_approvals").and_then(|v| v.as_array()).map(|a| a.len()))
        .unwrap_or(0);
    assert_eq!(pending, 0, "the second request never parked a row");
    runner.close();
    let _ = std::fs::remove_dir_all(&dir);
}
