//! R2-P4 R19 daemon end-to-end (scripted provider, real Unix socket): the
//! greeting handshake, a consistent checkpoint (snapshot + watermark in one
//! read), client-chosen command ids that dedup across reconnects, event
//! backfill after a watermark, lifecycle control and reads — the thin
//! client never executes anything itself (plan §9).

use serde_json::{json, Value as Json};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use teamagents_core::kernel::{KernelProfile, ModelRequest, ModelResponse, Usage};
use teamagents_core::models::UserConfig;
use teamagents_engine::providers::{AttemptOutcome, Cancel, Provider, ProviderError, ProviderEvent};
use teamagents_engine::v2::daemon::{serve, DaemonConfig, DaemonHandle, PROTOCOL_VERSION};
use teamagents_engine::v2::exec::{execute, End, ExecOptions, ExecRun};
use teamagents_engine::v2::supervisor::SupervisorConfig;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

struct ScriptedProvider {
    script: Mutex<VecDeque<Json>>,
}

impl Provider for ScriptedProvider {
    fn protocol(&self) -> &str {
        "scripted"
    }
    async fn complete(
        &self,
        _request: &ModelRequest,
        _cancel: &Cancel,
        _on_event: &mut (dyn FnMut(ProviderEvent) + Send),
    ) -> Result<AttemptOutcome, ProviderError> {
        let message = self.script.lock().unwrap().pop_front().unwrap_or_else(|| reply("script exhausted"));
        // {"__slow_ms__": N, …} answers after a delay so a test can act *while*
        // the request is in flight (the same trick the supervisor harness uses)
        if let Some(delay) = message["__slow_ms__"].as_u64() {
            let mut message = message;
            message.as_object_mut().unwrap().remove("__slow_ms__");
            tokio::time::sleep(Duration::from_millis(delay)).await;
            return Ok(AttemptOutcome {
                response: ModelResponse {
                    message,
                    usage: Some(Usage { prompt: 3, completion: 2, total: 5 }),
                    native: json!({}),
                },
                raw: json!({"scripted": true}),
                elapsed_ms: delay,
            });
        }
        // {"__error__": "…"} scripts a permanent provider failure
        if let Some(reason) = message["__error__"].as_str() {
            return Err(ProviderError::permanent(reason));
        }
        // {"__transient__": "…", "__retry_after_ms__": N} scripts the flaky-network failure the driver retries
        // inside the turn, with the provider's own Retry-After (D-117)
        if let Some(reason) = message["__transient__"].as_str() {
            let retry_after = message["__retry_after_ms__"].as_u64().map(Duration::from_millis);
            return Err(ProviderError { retry_after, ..ProviderError::transient(reason) });
        }
        Ok(AttemptOutcome {
            response: ModelResponse {
                message,
                usage: Some(Usage { prompt: 3, completion: 2, total: 5 }),
                native: json!({}),
            },
            raw: json!({"scripted": true}),
            elapsed_ms: 1,
        })
    }
}

fn reply(text: &str) -> Json {
    json!({"role": "assistant", "content": text})
}

fn shell_call(id: &str, command: &str) -> Json {
    json!({"role": "assistant", "content": "",
           "tool_calls": [{"id": id, "type": "function",
                           "function": {"name": "shell", "arguments": json!({"command": command}).to_string()}}]})
}

fn finish_call(summary: &str) -> Json {
    json!({"role": "assistant", "content": "",
           "tool_calls": [{"id": "finish-1", "type": "function",
                           "function": {"name": "finish",
                                        "arguments": json!({"status": "success", "summary": summary}).to_string()}}]})
}

mod common;

/// The scratch root of one test: `teamagents-v2-daemon-<tag>-<uuid>`, removed when the test ends (D-226).
type Root = common::TempRoot;

fn root(tag: &str) -> Root {
    Root::new(&format!("v2-daemon-{tag}"))
}

fn config(
    root: &Root,
    scripts: HashMap<String, Vec<Json>>,
) -> DaemonConfig<ScriptedProvider, impl Fn(&str, &KernelProfile) -> ScriptedProvider> {
    let scripts = Mutex::new(scripts);
    let factory = move |id: &str, _profile: &KernelProfile| {
        let script = scripts.lock().unwrap().remove(id).unwrap_or_default();
        ScriptedProvider { script: Mutex::new(script.into()) }
    };
    DaemonConfig {
        supervisor: SupervisorConfig {
            marker: std::marker::PhantomData,
            session_db: root.dir.join("session.sqlite"),
            session_id: "s-test".into(),
            leader_id: "i-leader".into(),
            leader_profile: KernelProfile {
                model: "scripted".into(),
                instructions: "team leader".into(),
                tools: vec![],
                options: json!({}),
                context_window: Some(128_000),
            },
            state_root: root.dir.join("state"),
            workspace: root.dir.join("ws"),
            permissions: "full_auto".into(),
            catalog: UserConfig::default(),
            bindings: vec![],
            max_retries: 2,
            storage_queue: 64,
            poll: Duration::from_millis(15),
            goal_limits: json!({}),
            require_shell_approval: false,
            provider_factory: factory,
        },
        socket: root.dir.join("state/daemon.sock"),
    }
}

struct Client {
    read: tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    write: tokio::net::unix::OwnedWriteHalf,
    greeting: Json,
}

impl Client {
    async fn connect(socket: &Path) -> Client {
        let stream = tokio::net::UnixStream::connect(socket).await.expect("connect");
        let (read, write) = stream.into_split();
        let mut read = tokio::io::BufReader::new(read);
        let mut line = String::new();
        read.read_line(&mut line).await.expect("greeting");
        let greeting: Json = serde_json::from_str(&line).expect("greeting JSON");
        Client { read, write, greeting }
    }

    async fn request(&mut self, frame: Json) -> Json {
        self.write.write_all(format!("{frame}\n").as_bytes()).await.expect("write");
        let mut line = String::new();
        self.read.read_line(&mut line).await.expect("reply");
        serde_json::from_str(&line).expect("reply JSON")
    }

    async fn call(&mut self, method: &str, params: Json) -> Json {
        self.request(json!({"protocol_version": PROTOCOL_VERSION,
                            "request_id": uuid::Uuid::new_v4().to_string(),
                            "method": method, "params": params}))
            .await
    }

    async fn command(&mut self, command_id: &str, method: &str, params: Json) -> Json {
        self.request(json!({"protocol_version": PROTOCOL_VERSION,
                            "request_id": uuid::Uuid::new_v4().to_string(),
                            "method": method, "command_id": command_id, "params": params}))
            .await
    }
}

/// Poll events after `since` until the goal completes; returns (status, watermark).
async fn wait_goal(client: &mut Client, since: i64) -> (String, i64) {
    for _ in 0..600 {
        let reply = client.call("events", json!({"since": since})).await;
        assert_eq!(reply["ok"], json!(true), "{reply}");
        if let Some(event) =
            reply["result"]["events"].as_array().unwrap().iter().find(|e| e["kind"] == json!("goal_completed"))
        {
            return (
                event["payload"]["status"].as_str().unwrap_or("").to_string(),
                reply["result"]["watermark"].as_i64().unwrap(),
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("goal not completed within 15s");
}

async fn boot(tag: &str, scripts: HashMap<String, Vec<Json>>) -> (Root, DaemonHandle) {
    let root = root(tag);
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let handle = serve(config(&root, scripts)).await.expect("daemon");
    (root, handle)
}

fn input_params(text: &str) -> Json {
    json!({"instance_id": "i-leader", "envelope_id": format!("env-{}", uuid::Uuid::new_v4()), "text": text})
}

#[tokio::test]
async fn handshake_checkpoint_command_and_goal_completion() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let scripts = HashMap::from([("i-leader".to_string(), vec![finish_call("done over the wire")])]);
    let (root, handle) = boot("e2e", scripts).await;
    let mut client = Client::connect(&root.dir.join("state/daemon.sock")).await;
    // greeting: version, session and state root (§9 handshake)
    assert_eq!(client.greeting["protocol_version"], json!(PROTOCOL_VERSION));
    assert_eq!(client.greeting["session_id"], json!("s-test"));
    assert!(client.greeting["state_root"].as_str().unwrap().ends_with("state"));
    // checkpoint: snapshot + watermark in one consistent read
    let checkpoint = client.call("checkpoint", json!({})).await;
    assert_eq!(checkpoint["ok"], json!(true), "{checkpoint}");
    let instances = checkpoint["result"]["snapshot"]["instances"].as_array().unwrap();
    assert!(instances.iter().any(|i| i["id"] == json!("i-leader")), "{instances:?}");
    let since = checkpoint["result"]["watermark"].as_i64().unwrap();
    // a client-chosen command id rides at the request level (§9)
    let params = input_params("do it");
    let submitted = client.command("cmd-first-input", "submit_input", params.clone()).await;
    assert_eq!(submitted["ok"], json!(true), "{submitted}");
    // replaying the identical frame returns the stored receipt, never twice;
    // the same id with a different payload is rejected instead (§6.3)
    let replayed = client.command("cmd-first-input", "submit_input", params).await;
    assert_eq!(replayed["ok"], json!(true));
    assert_eq!(replayed["result"], submitted["result"], "replay dedups by command id");
    let conflicted = client.command("cmd-first-input", "submit_input", input_params("different text")).await;
    assert_eq!(conflicted["ok"], json!(false), "same id, different payload must be rejected");
    let (status, _) = wait_goal(&mut client, since).await;
    assert_eq!(status, "SUCCEEDED");
    // reads: history of the leader holds the input and the finish
    let history = client.call("history", json!({"instance_id": "i-leader"})).await;
    let entries = history["result"]["entries"].as_array().unwrap();
    assert!(entries.iter().any(|e| e["kind"] == json!("user")), "{entries:?}");
    // the envelope id rides along (D-72), and the page is chronological
    let mine = entries.iter().find(|entry| entry["kind"] == json!("user")).expect("the input");
    assert!(mine["envelope_id"].as_str().is_some_and(|id| id.starts_with("env-")), "{mine}");
    assert!(entries.windows(2).all(|pair| pair[0]["idx"].as_i64() <= pair[1]["idx"].as_i64()), "{entries:?}");
    assert!(client.call("tasks", json!({})).await["ok"].as_bool().unwrap());
    assert!(client.call("grants", json!({})).await["ok"].as_bool().unwrap());
    // wrong protocol version is refused
    let bad = client
        .request(
            json!({"protocol_version": PROTOCOL_VERSION + 99, "request_id": "x", "method": "checkpoint", "params": {}}),
        )
        .await;
    assert_eq!(bad["ok"], json!(false));
    // a business method without a command id is refused
    let missing = client.call("submit_input", input_params("no id")).await;
    assert_eq!(missing["ok"], json!(false));
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn reconnect_backfills_events_after_the_watermark() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let scripts = HashMap::from([("i-leader".to_string(), vec![finish_call("first"), reply("second turn answered")])]);
    let (root, handle) = boot("reconnect", scripts).await;
    let mut client = Client::connect(&root.dir.join("state/daemon.sock")).await;
    let checkpoint = client.call("checkpoint", json!({})).await;
    let since = checkpoint["result"]["watermark"].as_i64().unwrap();
    client.command("cmd-input-a", "submit_input", input_params("first goal")).await;
    let (status, watermark) = wait_goal(&mut client, since).await;
    assert_eq!(status, "SUCCEEDED");
    drop(client); // the client goes away; the daemon keeps running (§9)
                  // reconnect: checkpoint again, then read everything after the watermark
    let mut client = Client::connect(&root.dir.join("state/daemon.sock")).await;
    let checkpoint = client.call("checkpoint", json!({})).await;
    assert_eq!(checkpoint["ok"], json!(true));
    let goal = &checkpoint["result"]["snapshot"]["goal"];
    assert_eq!(goal["status"], json!("SUCCEEDED"), "{checkpoint}");
    let events = client.call("events", json!({"since": 0})).await;
    let kinds: Vec<&str> =
        events["result"]["events"].as_array().unwrap().iter().filter_map(|e| e["kind"].as_str()).collect();
    assert!(kinds.contains(&"goal_completed"), "{kinds:?}");
    assert_eq!(events["result"]["resync_required"], json!(false));
    // lifecycle control through the same command surface
    let paused =
        client.command("cmd-pause", "set_lifecycle", json!({"instance_id": "i-leader", "lifecycle": "PAUSED"})).await;
    assert_eq!(paused["ok"], json!(true), "{paused}");
    let checkpoint = client.call("checkpoint", json!({})).await;
    let leader = checkpoint["result"]["snapshot"]["instances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == json!("i-leader"))
        .cloned()
        .unwrap();
    assert_eq!(leader["lifecycle"], json!("PAUSED"));
    let resumed =
        client.command("cmd-resume", "set_lifecycle", json!({"instance_id": "i-leader", "lifecycle": "ACTIVE"})).await;
    assert_eq!(resumed["ok"], json!(true), "{resumed}");
    let _ = watermark;
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn approvals_surface_lists_and_decides_pending_operations() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let scripts =
        HashMap::from([("i-leader".to_string(), vec![shell_call("c1", "true"), finish_call("approved and done")])]);
    let root = root("approvals");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let mut cfg = config(&root, scripts);
    cfg.supervisor.require_shell_approval = true;
    let handle = serve(cfg).await.expect("daemon");
    let mut client = Client::connect(&root.dir.join("state/daemon.sock")).await;
    let checkpoint = client.call("checkpoint", json!({})).await;
    let since = checkpoint["result"]["watermark"].as_i64().unwrap();
    let submitted = client.command("cmd-approval-input", "submit_input", input_params("run it")).await;
    assert_eq!(submitted["ok"], json!(true), "{submitted}");
    // the dispatch parks behind a PENDING approval (§9 approval handling)
    let mut approval_id = String::new();
    for _ in 0..400 {
        let events = client.call("events", json!({"since": since})).await;
        if let Some(event) =
            events["result"]["events"].as_array().unwrap().iter().find(|e| e["kind"] == json!("approval_requested"))
        {
            approval_id = event["payload"]["approval_id"].as_str().unwrap_or("").to_string();
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(!approval_id.is_empty(), "approval was never requested");
    let approvals = client.call("approvals", json!({})).await;
    assert_eq!(approvals["ok"], json!(true), "{approvals}");
    let pending = approvals["result"]["approvals"].as_array().unwrap();
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0]["id"], json!(approval_id));
    assert_eq!(pending[0]["tool"], json!("shell"));
    assert_eq!(pending[0]["preview"], json!("true"));
    // the decision rides the write surface with a stable command id
    let decided = client.command("cmd-approve-1", "approve", json!({"approval_id": approval_id})).await;
    assert_eq!(decided["ok"], json!(true), "{decided}");
    // no longer pending, and the goal eventually completes
    for _ in 0..100 {
        let approvals = client.call("approvals", json!({})).await;
        if approvals["result"]["approvals"].as_array().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let (status, _) = wait_goal(&mut client, since).await;
    assert_eq!(status, "SUCCEEDED");
    handle.shutdown().await.expect("shutdown");
}

/// The same decision through the **real binary** (D-67): `exec` reports a parked
/// approval and exits 3, and a headless user previously had to start the TUI or use
/// `--full-auto`. `teamagents approvals` lists it and decides it; the session then
/// dispatches the call the decision was bound to and the goal completes.
#[tokio::test]
async fn the_approvals_cli_lists_and_decides_a_parked_operation() {
    use std::process::Command;
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let scripts =
        HashMap::from([("i-leader".to_string(), vec![shell_call("c1", "true"), finish_call("approved from the cli")])]);
    let root = root("approvals-cli");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let mut cfg = config(&root, scripts);
    cfg.supervisor.require_shell_approval = true;
    let handle = serve(cfg).await.expect("daemon");
    let state_root = root.dir.join("state");
    let mut client = Client::connect(&state_root.join("daemon.sock")).await;
    let since = client.call("checkpoint", json!({})).await["result"]["watermark"].as_i64().unwrap();
    client.command("cli-approval-input", "submit_input", input_params("run it")).await;
    let mut approval_id = String::new();
    for _ in 0..400 {
        let events = client.call("events", json!({"since": since})).await;
        if let Some(event) =
            events["result"]["events"].as_array().unwrap().iter().find(|e| e["kind"] == json!("approval_requested"))
        {
            approval_id = event["payload"]["approval_id"].as_str().unwrap_or("").to_string();
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(!approval_id.is_empty(), "approval was never requested");
    // The client is a child process: run it on a blocking thread, or the test's
    // current-thread runtime (which is also serving the daemon) would be blocked
    // while the daemon must answer the child's handshake.
    let approvals = |args: Vec<String>, state: PathBuf| async move {
        tokio::task::spawn_blocking(move || {
            let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
                .args(&args)
                .arg("--state-root")
                .arg(&state)
                .output()
                .expect("run the approvals client");
            (
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stdout).into_owned(),
                String::from_utf8_lossy(&output.stderr).into_owned(),
            )
        })
        .await
        .expect("join the approvals client")
    };
    // listing shows the id, the tool and the exact call
    let (code, out, err) = approvals(vec!["approvals".into(), "--json".into()], state_root.clone()).await;
    assert_eq!(code, 0, "{err}");
    let listed: Json = serde_json::from_str(&out).expect("JSON report");
    assert_eq!(listed["approvals"][0]["id"], json!(approval_id), "{listed}");
    assert_eq!(listed["approvals"][0]["preview"], json!("true"), "{listed}");
    let (code, out, _) = approvals(vec!["approvals".into()], state_root.clone()).await;
    assert_eq!(code, 0);
    assert!(out.contains(&approval_id) && out.contains("shell") && out.contains("true"), "{out}");
    // a typo is a client error, not a decision about something else
    let (code, _, err) =
        approvals(vec!["approvals".into(), "approve".into(), "--id".into(), "ap-nope".into()], state_root.clone())
            .await;
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("no approval id starts with"), "{err}");
    // and the decision goes through the same write surface the TUI uses
    let (code, out, err) = approvals(
        vec!["approvals".into(), "approve".into(), "--id".into(), approval_id.clone(), "--json".into()],
        state_root.clone(),
    )
    .await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(serde_json::from_str::<Json>(&out).expect("JSON")["decision"], json!("approve"), "{out}");
    let (status, _) = wait_goal(&mut client, since).await;
    assert_eq!(status, "SUCCEEDED", "the approved call was dispatched and the turn continued");
    let (code, out, _) = approvals(vec!["approvals".into()], state_root.clone()).await;
    assert_eq!(code, 0);
    assert!(out.contains("0 pending approval(s)"), "{out}");
    handle.shutdown().await.expect("shutdown");
}

/// The §5.4 levers through the real binary (D-68): a delegator parked on a task whose
/// assignee stopped without settling it (D-65) is released by `teamagents tasks
/// cancel`, and the instance levers really change the lifecycle. This is the workflow
/// the operating notes describe ("cancel it in the tasks panel"), reachable without a
/// TUI.
#[tokio::test]
async fn the_intervention_cli_cancels_a_task_and_pauses_and_resumes_an_instance() {
    use std::process::Command;
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    // the leader delegates and waits; the worker answers with prose and never settles
    let leader = vec![
        json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": "spawn",
             "arguments": json!({"instance_id": "i-worker", "instructions": "do it"}).to_string()}},
            {"id": "c2", "type": "function", "function": {"name": "delegate",
             "arguments": json!({"assignee": "i-worker", "task_id": "t-prose", "description": "write it"}).to_string()}}
        ]}),
        json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "c3", "type": "function", "function": {"name": "wait",
             "arguments": json!({"mode": "ANY", "conditions": [{"kind": "task", "task_id": "t-prose"}]}).to_string()}}]}),
        finish_call("the task was cancelled"),
    ];
    let worker = vec![reply("I have not finished; see my notes above.")];
    let scripts = HashMap::from([("i-leader".to_string(), leader), ("i-worker".to_string(), worker)]);
    let root = root("intervention-cli");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let handle = serve(config(&root, scripts)).await.expect("daemon");
    let state_root = root.dir.join("state");
    let mut client = Client::connect(&state_root.join("daemon.sock")).await;
    let since = client.call("checkpoint", json!({})).await["result"]["watermark"].as_i64().unwrap();
    client.command("intervention-input", "submit_input", input_params("delegate it")).await;
    // the worker replies with prose: its turn ends, the task stays RUNNING, the leader waits
    let mut running = false;
    for _ in 0..800 {
        let tasks = client.call("tasks", json!({})).await;
        if tasks["result"]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|task| task["id"] == json!("t-prose") && task["status"] == json!("RUNNING"))
        {
            running = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(running, "the delegated task is RUNNING while its assignee stopped");
    let cli = |args: Vec<String>, state: PathBuf| async move {
        tokio::task::spawn_blocking(move || {
            let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
                .args(&args)
                .arg("--state-root")
                .arg(&state)
                .output()
                .expect("run the client");
            (
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stdout).into_owned(),
                String::from_utf8_lossy(&output.stderr).into_owned(),
            )
        })
        .await
        .expect("join the client")
    };
    // listing shows the task and the instance states
    let (code, out, err) = cli(vec!["tasks".into()], state_root.clone()).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("t-prose") && out.contains("RUNNING"), "{out}");
    let (code, out, _) = cli(vec!["instances".into()], state_root.clone()).await;
    assert_eq!(code, 0);
    assert!(out.contains("i-worker") && out.contains("ACTIVE"), "{out}");
    // termination is deliberate: it needs --yes
    let (code, _, err) =
        cli(vec!["instances".into(), "terminate".into(), "--id".into(), "i-worker".into()], state_root.clone()).await;
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("--yes"), "{err}");
    // pausing and resuming really move the lifecycle
    let (code, out, err) =
        cli(vec!["instances".into(), "pause".into(), "--id".into(), "i-worker".into()], state_root.clone()).await;
    assert_eq!(code, 0, "{err}");
    // D-177: the line is the row *after* the lever, in the shape `instances` prints — it used to put the new
    // lifecycle beside the pre-change row ("PAUSED i-worker: ACTIVE / …"), which read as a contradiction
    assert!(out.contains("i-worker: PAUSED /"), "{out}");
    assert!(!out.contains("PAUSED i-worker:"), "{out}");
    let (code, out, err) =
        cli(vec!["instances".into(), "resume".into(), "--id".into(), "i-worker".into()], state_root.clone()).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("i-worker: ACTIVE /"), "{out}");
    // cancelling the task releases the delegator: the goal settles
    let (code, out, err) =
        cli(vec!["tasks".into(), "cancel".into(), "--id".into(), "t-prose".into()], state_root.clone()).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("cancelled t-prose"), "{out}");
    let (status, _) = wait_goal(&mut client, since).await;
    assert_eq!(status, "SUCCEEDED", "the delegator woke on the cancellation and settled");
    let tasks = client.call("tasks", json!({})).await;
    assert_eq!(tasks["result"]["tasks"][0]["status"], json!("CANCELLED"), "{tasks}");
    handle.shutdown().await.expect("shutdown");
}

/// Every instance's row describes the model it runs on (D-69), and the snapshot
/// reports it: a team can span providers (`spawn(model = …)`, D-59), and without this
/// no client — the TUI included — could say which member runs on what. The stored
/// name is the *resolved* one (a catalog key becomes the wire model), matching what a
/// spawned child stores.
#[tokio::test]
async fn the_snapshot_reports_each_members_model() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("member-model");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let mut cfg = config(&root, HashMap::new());
    // the session boots on a catalog entry: the key is "leader_main", the wire model
    // is "deepseek-flash", and the row must carry the latter
    cfg.supervisor.leader_profile = KernelProfile {
        model: "leader_main".into(),
        instructions: "team leader".into(),
        tools: vec![],
        options: json!({}),
        context_window: Some(128_000),
    };
    let mut catalog = UserConfig::default();
    catalog.models.insert(
        "leader_main".into(),
        serde_json::from_value(json!({"provider": "deepseek", "protocol": "deepseek",
                                      "model": "deepseek-flash", "context_window": 1000000}))
        .expect("a model profile"),
    );
    cfg.supervisor.catalog = catalog;
    let handle = serve(cfg).await.expect("daemon");
    let mut client = Client::connect(&root.dir.join("state/daemon.sock")).await;
    let checkpoint = client.call("checkpoint", json!({})).await;
    let instances = checkpoint["result"]["snapshot"]["instances"].as_array().cloned().unwrap_or_default();
    assert_eq!(instances.len(), 1, "{checkpoint}");
    assert_eq!(instances[0]["id"], json!("i-leader"));
    assert_eq!(instances[0]["model"], json!("deepseek-flash"), "the resolved model, not the catalog key: {checkpoint}");
    // and it is persisted, so any reader (not only a snapshot) sees it
    let stored: String = teamagents_core::v2::Control::open(&root.dir.join("session.sqlite"), "s-test", false)
        .expect("control")
        .connection()
        .query_row("SELECT profile_json FROM instances WHERE id = 'i-leader'", [], |row| row.get(0))
        .expect("profile row");
    assert!(stored.contains("deepseek-flash"), "{stored}");
    handle.shutdown().await.expect("shutdown");
}

/// A33: one coordinator per state root (jobs::state_lock) — a second daemon
/// is refused while the first runs; after shutdown and client disconnect
/// the kernel releases the lock and a new coordinator recovers the session.
#[tokio::test]
async fn second_daemon_is_refused_and_shutdown_releases_the_lock() {
    let root = root("lock");
    let socket = root.dir.join("state/daemon.sock");
    let handle = serve(config(&root, HashMap::new())).await.expect("first daemon");
    let second = serve(config(&root, HashMap::new())).await;
    let error = second.err().map(|e| e.to_string()).unwrap_or_else(|| "second daemon unexpectedly booted".into());
    assert!(error.contains("already has a coordinator"), "{error}");
    // the refused attempt did not disturb the running coordinator
    let mut client = Client::connect(&socket).await;
    let reply = client.call("checkpoint", json!({})).await;
    assert!(reply["result"].get("snapshot").is_some(), "{reply}");
    drop(client);
    handle.shutdown().await.expect("shutdown");
    // connection tasks release their supervisor refs asynchronously; poll
    let mut recovered = None;
    for _ in 0..50 {
        match serve(config(&root, HashMap::new())).await {
            Ok(handle) => {
                recovered = Some(handle);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    recovered.expect("lock released for recovery").shutdown().await.expect("shutdown");
}

// ---------------------------------------------------------------------------
// `teamagents exec`: the headless client contract (D-32/D-49) driven against a
// real socket, a real session database and a scripted leader.
// ---------------------------------------------------------------------------

fn exec_options(socket: &Path, workspace: &Path, prompt: &str, checks: Vec<String>) -> ExecOptions {
    ExecOptions {
        socket: socket.to_path_buf(),
        prompt: prompt.to_string(),
        timeout_s: 60,
        json_out: true,
        stream_events: false,
        checks,
        workspace: workspace.to_path_buf(),
    }
}

/// `execute` is a blocking client: run it off the runtime's async threads.
async fn headless_result(options: ExecOptions) -> Result<ExecRun, (i32, String)> {
    tokio::task::spawn_blocking(move || execute(&options)).await.expect("exec task")
}

async fn headless(options: ExecOptions) -> ExecRun {
    headless_result(options).await.expect("headless run")
}

/// The run reports its own outcome: a settled goal carries the real goal status
/// and exit code 0, and the settlement stored by an *earlier* run is history
/// (the client drains the event log before submitting), never this run's result.
#[tokio::test]
async fn headless_runs_report_their_own_outcome_not_an_earlier_settlement() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let scripts =
        HashMap::from([("i-leader".to_string(), vec![finish_call("done over the wire"), reply("a plain answer")])]);
    let (root, handle) = boot("exec-outcome", scripts).await;
    let (socket, workspace) = (root.dir.join("state/daemon.sock"), root.dir.join("ws"));
    let first = headless(exec_options(&socket, &workspace, "finish it", Vec::new())).await;
    assert_eq!(first.end, End::Completed, "{}", first.report);
    assert_eq!(first.report["goal_status"], json!("SUCCEEDED"));
    assert_eq!(first.report["end"], json!("completed"));
    assert_eq!(first.end.exit_code(first.checks_ok), 0);
    assert!(first.report["watermark"].as_i64().unwrap() > 0, "{}", first.report);
    // the goal is settled for good: the next run ends on a plain reply, and the
    // stored `goal_completed` event is not replayed as its outcome
    let second = headless(exec_options(&socket, &workspace, "just answer", Vec::new())).await;
    assert_eq!(second.end, End::Reply, "{}", second.report);
    assert_eq!(second.report["reply"], json!("a plain answer"));
    assert_eq!(second.end.exit_code(second.checks_ok), 0);
    handle.shutdown().await.expect("shutdown");
}

/// D-275: a settlement is read against the goal this run's turn was working on, not against the session's single
/// goal. A session carrying an *older* ACTIVE goal beside the run's own (D-267/D-268) makes the checkpoint report
/// that older goal once the run's goal settles, so the client read its own committed settlement as
/// not-yet-committed and waited out the caller's deadline — the worst kind of wrong answer for a CI job
/// (measured in this repository's self-refine session, 2026-09-28: `goal-s-main` ACTIVE and detached beside a
/// settled `goal-task2`, and the run was still waiting fourteen minutes later). The runtime's closing entry
/// names its goal (`goal-close-<goal_id>`), so the client can confirm the settlement without touching the
/// checkpoint's single-goal semantics.
#[tokio::test]
async fn a_settlement_is_read_against_the_runs_own_goal_not_the_sessions_active_one() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let scripts = HashMap::from([("i-leader".to_string(), vec![finish_call("done over the wire")])]);
    let (root, handle) = boot("exec-two-goals", scripts).await;
    let (socket, workspace) = (root.dir.join("state/daemon.sock"), root.dir.join("ws"));
    // the boot goal (`goal-s-test`) stays ACTIVE and detached when a second goal attaches to the leader: that is
    // the goal the checkpoint reports once this run's own goal has settled, and it is not this run's outcome
    let mut client = Client::connect(&socket).await;
    let opened =
        client.command("open-run-goal", "create_goal", json!({"id": "g-run", "instance_id": "i-leader"})).await;
    assert_eq!(opened["ok"], json!(true), "{opened}");
    let checkpoint = client.call("checkpoint", json!({})).await;
    assert_eq!(checkpoint["result"]["snapshot"]["goal"]["status"], json!("ACTIVE"), "{checkpoint}");
    let options = ExecOptions { timeout_s: 10, ..exec_options(&socket, &workspace, "finish it", Vec::new()) };
    let started = std::time::Instant::now();
    let run = headless(options).await;
    assert_eq!(run.end, End::Completed, "{}", run.report);
    assert_eq!(run.report["goal_status"], json!("SUCCEEDED"), "{}", run.report);
    assert_eq!(run.report["end"], json!("completed"));
    assert_eq!(run.end.exit_code(run.checks_ok), 0);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the settlement is reported, not waited out ({}s)",
        started.elapsed().as_secs()
    );
    // the session still carries the older goal, and it is still ACTIVE: the state the client had to look past
    let goals = client.call("goals", json!({})).await;
    let rows = goals["result"]["goals"].as_array().cloned().unwrap_or_default();
    assert!(rows.iter().any(|goal| goal["id"] == json!("g-run") && goal["status"] == json!("SUCCEEDED")), "{goals}");
    assert!(rows.iter().any(|goal| goal["id"] == json!("goal-s-test") && goal["status"] == json!("ACTIVE")), "{goals}");
    handle.shutdown().await.expect("shutdown");
}

/// D-277: `instances resume` is a user *action*, and a user action is one logical operation per invocation. Its
/// command id was fixed per (instance, lifecycle) — `lifecycle-<id>-<lifecycle>` — and the control plane dedups
/// by command id, so the *second* resume returned the first one's recorded reply and printed it as a fresh
/// success (`i-leader: ACTIVE / READY`) while the instance stayed parked: the lever `exec` itself tells a stuck
/// user to run (`instances resume --id i-leader`), lying the second time it is used (measured 2026-09-28, D-276's
/// phase, on a session whose leader had parked twice).
///
/// The state between the two invocations is set by the *test's* own fresh `set_lifecycle` command, never by the
/// lever under test: a park is cleared by a resume, but the resumed turn re-runs its interrupted request and parks
/// again within milliseconds, so the state right after a resume is not a stable observation. Driving the real
/// binary against a real daemon this way is deterministic, needs no model call, and asserts the *effect* of the
/// second invocation rather than the sentence it prints.
#[tokio::test]
async fn a_second_resume_is_a_new_operation_not_the_first_ones_recorded_reply() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let (root, handle) = boot("exec-resume-twice", HashMap::new()).await;
    let state = root.dir.join("state");
    let socket = state.join("daemon.sock");
    // the real binary, the user's own lever, off the runtime's thread (the daemon serves it on this runtime)
    let lever = |verb: &str| {
        let state = state.clone();
        let verb = verb.to_string();
        tokio::task::spawn_blocking(move || {
            let output = std::process::Command::new(env!("CARGO_BIN_EXE_teamagents"))
                .args(["instances", &verb, "--id", "i-leader", "--state-root"])
                .arg(&state)
                .output()
                .expect("run the instances lever");
            (output.status.code(), String::from_utf8_lossy(&output.stdout).into_owned())
        })
    };
    async fn leader_lifecycle(socket: &Path) -> String {
        let mut client = Client::connect(socket).await;
        let checkpoint = client.call("checkpoint", json!({})).await;
        checkpoint["result"]["snapshot"]["instances"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["id"] == json!("i-leader")))
            .and_then(|row| row["lifecycle"].as_str())
            .unwrap_or("")
            .to_string()
    }
    let mut setup = Client::connect(&socket).await;
    async fn set(client: &mut Client, lifecycle: &str) -> Json {
        let command_id = format!("setup-{lifecycle}-{}", uuid::Uuid::new_v4());
        let params = json!({"instance_id": "i-leader", "lifecycle": lifecycle, "reason": "test setup"});
        client.command(&command_id, "set_lifecycle", params).await
    }

    // `resume`: the same instance is resumed twice, with the lever's own state read in between
    let paused = set(&mut setup, "PAUSED").await;
    assert_eq!(paused["ok"], json!(true), "{paused}");
    assert_eq!(leader_lifecycle(&socket).await, "PAUSED", "the setup transition is carried out");

    let (code, printed) = lever("resume").await.expect("resume task");
    assert_eq!(code, Some(0), "{printed}");
    assert!(printed.contains("i-leader: ACTIVE"), "{printed}");
    assert_eq!(leader_lifecycle(&socket).await, "ACTIVE", "the first resume is carried out");

    let paused = set(&mut setup, "PAUSED").await;
    assert_eq!(paused["ok"], json!(true), "{paused}");
    assert_eq!(leader_lifecycle(&socket).await, "PAUSED", "the instance is not ACTIVE again");
    let (code, printed) = lever("resume").await.expect("resume task");
    assert_eq!(code, Some(0), "{printed}");
    assert!(printed.contains("i-leader: ACTIVE"), "the sentence a user reads: {printed}");
    assert_eq!(
        leader_lifecycle(&socket).await,
        "ACTIVE",
        "the second resume must be a new command, not the first one's recorded reply ({printed})"
    );

    // `pause` is the same shape: pause → resume → pause has to really pause
    let (code, printed) = lever("pause").await.expect("pause task");
    assert_eq!(code, Some(0), "{printed}");
    assert_eq!(leader_lifecycle(&socket).await, "PAUSED", "the first pause is carried out ({printed})");
    let active = set(&mut setup, "ACTIVE").await;
    assert_eq!(active["ok"], json!(true), "{active}");
    assert_eq!(leader_lifecycle(&socket).await, "ACTIVE", "the instance runs again");
    let (code, printed) = lever("pause").await.expect("pause task");
    assert_eq!(code, Some(0), "{printed}");
    assert!(printed.contains("i-leader: PAUSED"), "the sentence a user reads: {printed}");
    assert_eq!(
        leader_lifecycle(&socket).await,
        "PAUSED",
        "the second pause must be a new command, not the first one's recorded reply ({printed})"
    );
    handle.shutdown().await.expect("shutdown");
}

/// A goal the *runtime* blocks (its required checks never pass) is not a success
/// either: `exec` must report it and exit non-zero. The runtime's own block note is
/// what the instance stops on — an assistant-shaped one was exactly what a naive
/// "the last assistant entry is the reply" reading mistook for the model's answer —
/// and the block emitted no settlement event at all (D-71).
#[tokio::test]
async fn a_runtime_blocked_goal_is_not_reported_as_a_reply() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    // three finishes: each one runs the check round, the round never passes, the
    // third exhausts the driver's default repair budget and the runtime blocks the
    // goal (a fourth step would mean the runtime asked again — the storm, not this)
    let scripts = HashMap::from([(
        "i-leader".to_string(),
        vec![finish_call("done once"), finish_call("done twice"), finish_call("done thrice")],
    )]);
    let root = root("exec-runtime-blocked");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let mut cfg = config(&root, scripts);
    cfg.supervisor.goal_limits = json!({"required_checks": [{"id": "never", "command": "false", "timeout": 30}]});
    let handle = serve(cfg).await.expect("daemon");
    let (socket, workspace) = (root.dir.join("state/daemon.sock"), root.dir.join("ws"));
    let run = headless(exec_options(&socket, &workspace, "finish it", Vec::new())).await;
    assert_eq!(run.end, End::Failed, "{}", run.report);
    assert_eq!(run.report["goal_status"], json!("BLOCKED"), "{}", run.report);
    assert_eq!(run.report["reply"], Json::Null, "the runtime's own note is not a reply: {}", run.report);
    assert_eq!(run.end.exit_code(run.checks_ok), 1);
    // and the settlement is visible to any client, not only to the one that was polling
    let mut client = Client::connect(&socket).await;
    let events = client.call("events", json!({"since": 0})).await;
    let kinds: Vec<&str> =
        events["result"]["events"].as_array().unwrap().iter().filter_map(|event| event["kind"].as_str()).collect();
    assert!(kinds.contains(&"goal_completed"), "the block must be an event too: {kinds:?}");
    let completed = events["result"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["kind"] == json!("goal_completed"))
        .unwrap();
    assert_eq!(completed["payload"]["status"], json!("BLOCKED"), "{completed}");
    assert_eq!(completed["payload"]["blocked_by"], json!("runtime"), "{completed}");
    // what the instance stopped on is the runtime's word, by kind: any reader of
    // the conversation can tell it apart from the member's answer
    let history = client.call("history", json!({"instance_id": "i-leader", "limit": 200})).await;
    let entries = history["result"]["entries"].as_array().cloned().unwrap_or_default();
    let tail = entries.last().expect("a closing entry");
    assert_eq!(tail["kind"], json!("runtime"), "{tail}");
    assert_eq!(tail["message"]["role"], json!("user"), "{tail}");
    handle.shutdown().await.expect("shutdown");
}

/// A goal that settles as BLOCKED is not a success: the run reports it and
/// exits 1, so a CI job cannot read a blocked goal as delivered work.
#[tokio::test]
async fn a_blocked_goal_is_not_reported_as_a_success() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let blocked = json!({"role": "assistant", "content": "",
        "tool_calls": [{"id": "finish-1", "type": "function",
                        "function": {"name": "finish",
                                     "arguments": json!({"status": "blocked", "summary": "needs a decision"}).to_string()}}]});
    let scripts = HashMap::from([("i-leader".to_string(), vec![blocked])]);
    let (root, handle) = boot("exec-blocked", scripts).await;
    let (socket, workspace) = (root.dir.join("state/daemon.sock"), root.dir.join("ws"));
    let run = headless(exec_options(&socket, &workspace, "try it", Vec::new())).await;
    assert_eq!(run.end, End::Failed, "{}", run.report);
    assert_eq!(run.report["goal_status"], json!("BLOCKED"));
    assert_eq!(run.end.exit_code(run.checks_ok), 1);
    handle.shutdown().await.expect("shutdown");
}

/// A turn the runtime closes with nothing to settle is not the member's reply:
/// the goal settled in an earlier run, so this run's `finish` only ends the turn
/// (`close_completion`) and the runtime says so. Reporting that closing word as a
/// reply would be a success the run never had; waiting for the deadline would
/// call a finished turn a timeout (D-71).
#[tokio::test]
async fn a_turn_closed_by_the_runtime_without_a_settlement_is_not_a_reply() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let scripts = HashMap::from([(
        "i-leader".to_string(),
        vec![finish_call("done over the wire"), finish_call("finished again, nothing left to settle")],
    )]);
    let (root, handle) = boot("exec-runtime-closed", scripts).await;
    let (socket, workspace) = (root.dir.join("state/daemon.sock"), root.dir.join("ws"));
    let first = headless(exec_options(&socket, &workspace, "finish it", Vec::new())).await;
    assert_eq!(first.end, End::Completed, "{}", first.report);
    // the second run's goal is already terminal: its finish closes the turn
    let options = ExecOptions { timeout_s: 30, ..exec_options(&socket, &workspace, "finish again", Vec::new()) };
    let started = std::time::Instant::now();
    let second = headless(options).await;
    assert_eq!(second.end, End::Unsettled, "{}", second.report);
    assert_eq!(second.report["end"], json!("unsettled"));
    assert_eq!(second.report["goal_status"], Json::Null, "{}", second.report);
    assert_eq!(second.report["reply"], Json::Null, "the runtime's note is not a reply: {}", second.report);
    assert_eq!(second.end.exit_code(second.checks_ok), 1);
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "a closed turn must be reported, not waited out ({}s)",
        started.elapsed().as_secs()
    );
    handle.shutdown().await.expect("shutdown");
}

/// Termination is final: a terminated leader cannot be resumed (the control plane
/// refuses `set_lifecycle` on it, and the CLI's own `instances resume` says so), so
/// `exec` must not send the user to the resume key it used to name for every
/// non-ACTIVE lifecycle.
#[tokio::test]
async fn a_terminated_leader_is_reported_as_final_not_resumable() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let (root, handle) = boot("exec-terminated-leader", HashMap::new()).await;
    let socket = root.dir.join("state/daemon.sock");
    let mut client = Client::connect(&socket).await;
    let terminated = client
        .command(
            "term-1",
            "set_lifecycle",
            json!({"instance_id": "i-leader", "lifecycle": "TERMINATED", "reason": "test"}),
        )
        .await;
    assert_eq!(terminated["ok"], json!(true), "{terminated}");
    // the resume the old message pointed at is exactly what the session refuses
    let refused = client
        .command(
            "resume-1",
            "set_lifecycle",
            json!({"instance_id": "i-leader", "lifecycle": "ACTIVE", "reason": "test"}),
        )
        .await;
    assert_eq!(refused["ok"], json!(false), "{refused}");
    assert!(refused["error"].as_str().unwrap_or("").contains("terminated"), "{refused}");

    let error = headless_result(exec_options(&socket, &root.dir.join("ws"), "say hi", Vec::new()))
        .await
        .err()
        .expect("a terminated leader refuses input");
    assert_eq!(error.0, 2, "{error:?}");
    assert!(error.1.contains("TERMINATED") && error.1.contains("fresh state root"), "{error:?}");
    assert!(!error.1.contains("Resume"), "termination is final, so no resume advice: {error:?}");
    handle.shutdown().await.expect("shutdown");
}

/// A paused (or parked) leader can be resumed, and a headless client must name a lever
/// the caller can actually pull: the CLI verb D-68 added, not only a TUI key (D-82).
#[tokio::test]
async fn a_paused_leader_refusal_names_the_cli_lever() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let (root, handle) = boot("exec-paused-leader", HashMap::new()).await;
    let socket = root.dir.join("state/daemon.sock");
    let mut client = Client::connect(&socket).await;
    let paused = client
        .command(
            "pause-1",
            "set_lifecycle",
            json!({"instance_id": "i-leader", "lifecycle": "PAUSED", "reason": "test"}),
        )
        .await;
    assert_eq!(paused["ok"], json!(true), "{paused}");
    let error = headless_result(exec_options(&socket, &root.dir.join("ws"), "say hi", Vec::new()))
        .await
        .err()
        .expect("a paused leader refuses input");
    assert_eq!(error.0, 2, "{error:?}");
    assert!(error.1.contains("PAUSED"), "{error:?}");
    assert!(
        error.1.contains("instances resume --id i-leader"),
        "the caller is told the lever that works headlessly: {error:?}"
    );
    assert!(!error.1.contains("termination is final"), "a pause is not final: {error:?}");
    handle.shutdown().await.expect("shutdown");
}

/// A permanently failed leader request ends the headless run at once with exit
/// 1 and the classified reason: a broken endpoint must not look like a hung
/// session that the caller discovers when its own deadline expires.
#[tokio::test]
async fn a_failed_turn_ends_the_headless_run_instead_of_timing_out() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let scripts =
        HashMap::from([("i-leader".to_string(), vec![json!({"__error__": "chat API 401: invalid api key"})])]);
    let (root, handle) = boot("exec-turn-failure", scripts).await;
    let (socket, workspace) = (root.dir.join("state/daemon.sock"), root.dir.join("ws"));
    let options = ExecOptions { timeout_s: 120, ..exec_options(&socket, &workspace, "do it", Vec::new()) };
    let started = std::time::Instant::now();
    let run = headless(options).await;
    assert_eq!(run.end, End::Failed, "{}", run.report);
    assert_eq!(run.end.exit_code(run.checks_ok), 1);
    assert!(
        run.report["failure"].as_str().is_some_and(|reason| reason.contains("401")),
        "the classified reason is reported: {}",
        run.report
    );
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "a permanent model error must not wait for the deadline ({}s)",
        started.elapsed().as_secs()
    );
    // The failure parked the leader (§8 bounded handling). A parked leader will
    // not run new input, so the next run says so at once instead of queueing
    // work that nobody drains until the caller's deadline.
    let parked = headless_result(exec_options(&socket, &workspace, "again", Vec::new()))
        .await
        .err()
        .expect("a parked leader refuses new input");
    assert_eq!(parked.0, 2, "{parked:?}");
    assert!(parked.1.contains("PARKED") && parked.1.contains("nothing was submitted"), "{parked:?}");
    handle.shutdown().await.expect("shutdown");
}

/// The user's `--check` acceptance commands run in the client's workspace
/// through the isolated shell and gate the exit code; the ledger lands next to
/// the session database as the artifact a CI job archives.
///
/// Without bubblewrap (the GitHub runner, D-113) the ledger cannot show a pass, and the claim that *is*
/// observable there is asserted instead of skipped: a check that cannot run fails closed — the run reports it as
/// a failure, the ledger row keeps the isolation reason, and the exit code is 1.
#[tokio::test]
async fn headless_runs_verify_the_acceptance_commands_and_gate_the_exit_code() {
    let have_sandbox = teamagents_engine::tools::sandbox_usable();
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let scripts = HashMap::from([("i-leader".to_string(), vec![finish_call("done")])]);
    let (root, handle) = boot("exec-checks", scripts).await;
    let (socket, workspace) = (root.dir.join("state/daemon.sock"), root.dir.join("ws"));
    let passed = headless(exec_options(&socket, &workspace, "finish it", vec!["echo checked".to_string()])).await;
    assert_eq!(passed.end, End::Completed, "{}", passed.report);
    let verdicts = passed.report["verification"].as_array().unwrap();
    assert_eq!(verdicts.len(), 1, "{}", passed.report);
    if have_sandbox {
        assert!(passed.checks_ok, "{}", passed.report);
        assert_eq!(passed.end.exit_code(passed.checks_ok), 0);
        assert_eq!(verdicts[0]["ok"], json!(true));
        assert_eq!(verdicts[0]["exit_code"], json!(0));
        assert!(verdicts[0]["output"].as_str().unwrap().contains("checked"), "{}", passed.report);
    } else {
        assert!(!passed.checks_ok, "a check that cannot run is not a pass: {}", passed.report);
        assert_eq!(passed.end.exit_code(passed.checks_ok), 1);
        assert_eq!(verdicts[0]["ok"], json!(false));
        assert_eq!(verdicts[0]["exit_code"], json!(-1));
        let error = verdicts[0]["error"].as_str().unwrap_or("");
        assert!(error.contains("IsolationUnavailable"), "the refusal names the isolation: {}", passed.report);
    }
    let ledger = passed.report["verification_path"].as_str().expect("ledger path");
    assert_eq!(Path::new(ledger), root.dir.join("state/verification.json"));
    let written: Json = serde_json::from_str(&std::fs::read_to_string(ledger).unwrap()).unwrap();
    assert_eq!(written["verification"][0]["command"], json!("echo checked"));
    handle.shutdown().await.expect("shutdown");
}

/// A failing acceptance command turns a nominal success into exit 1 and stops
/// the remaining commands (later ones may depend on earlier ones).
///
/// Without bubblewrap (D-113) the same two properties are asserted through the refusal: the list stops at the
/// first check that cannot run, and the run is an honest failure rather than a pass over an unchecked goal.
#[tokio::test]
async fn a_failing_acceptance_command_fails_the_run() {
    let have_sandbox = teamagents_engine::tools::sandbox_usable();
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let scripts = HashMap::from([("i-leader".to_string(), vec![finish_call("done")])]);
    let (root, handle) = boot("exec-checks-fail", scripts).await;
    let (socket, workspace) = (root.dir.join("state/daemon.sock"), root.dir.join("ws"));
    let checks = vec!["echo broken; exit 7".to_string(), "echo never".to_string()];
    let run = headless(exec_options(&socket, &workspace, "finish it", checks)).await;
    assert_eq!(run.end, End::Completed, "the goal itself did settle: {}", run.report);
    assert!(!run.checks_ok);
    assert_eq!(run.end.exit_code(run.checks_ok), 1, "a failed acceptance is not a success");
    let verdicts = run.report["verification"].as_array().unwrap();
    assert_eq!(verdicts.len(), 1, "the first failure stops the list: {}", run.report);
    if have_sandbox {
        assert_eq!(verdicts[0]["exit_code"], json!(7));
        assert!(verdicts[0]["output"].as_str().unwrap().contains("broken"), "{}", run.report);
    } else {
        assert_eq!(verdicts[0]["ok"], json!(false));
        assert_eq!(verdicts[0]["exit_code"], json!(-1));
        let error = verdicts[0]["error"].as_str().unwrap_or("");
        assert!(error.contains("IsolationUnavailable"), "the refusal names the isolation: {}", run.report);
    }
    handle.shutdown().await.expect("shutdown");
}

/// A settlement is not a tool call, so the fixture spells the slow one out.
fn slow_finish(delay_ms: u64, summary: &str) -> Json {
    json!({"__slow_ms__": delay_ms, "role": "assistant", "content": "",
           "tool_calls": [{"id": "finish-slow", "type": "function", "function": {"name": "finish",
                           "arguments": json!({"status": "success", "summary": summary}).to_string()}}]})
}

/// Start a turn (the first step is slow, so it stays in flight) and hand back the
/// client, waited until the request is really in flight.
async fn start_a_turn_in_flight(client: &mut Client, text: &str) {
    let started = client.command("in-1", "submit_input", input_params(text)).await;
    assert_eq!(started["result"]["applied"], json!(true), "{started}");
    for _ in 0..200 {
        let checkpoint = client.call("checkpoint", json!({})).await;
        if checkpoint["result"]["snapshot"]["instances"][0]["phase"] == json!("MODEL_PENDING") {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the first turn never reached MODEL_PENDING");
}

/// An input that arrives while a turn is in flight is queued (D-63). Its run is
/// *its own* (D-49's "own outcome only", applied to the queued case): the goal the
/// earlier turn settled on its way out must not be reported as this input's
/// outcome — the input was not part of that goal, and before D-72 `exec` reported
/// `end: completed` with the earlier turn's settlement and exited 0.
#[tokio::test]
async fn a_queued_input_is_not_answered_by_the_previous_turns_settlement() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let scripts = HashMap::from([(
        "i-leader".to_string(),
        vec![slow_finish(1200, "the first task"), reply("answer to the queued question")],
    )]);
    let (root, handle) = boot("exec-queued-settle", scripts).await;
    let socket = root.dir.join("state/daemon.sock");
    let mut client = Client::connect(&socket).await;
    start_a_turn_in_flight(&mut client, "first question").await;
    // `exec` submits while that turn runs: its input queues behind it
    let run = headless(exec_options(&socket, &root.dir.join("ws"), "second question", Vec::new())).await;
    assert_eq!(run.report["input_queued"], json!(true), "{}", run.report);
    assert_eq!(run.end, End::Reply, "the run's own turn answered it: {}", run.report);
    assert_eq!(run.report["reply"], json!("answer to the queued question"), "{}", run.report);
    assert_eq!(
        run.report["goal_status"],
        Json::Null,
        "the earlier turn's settlement is not this run's outcome: {}",
        run.report
    );
    assert_eq!(run.end.exit_code(run.checks_ok), 0);
    // the session's goal really did settle — as the *first* turn's outcome
    let settled = client.call("checkpoint", json!({})).await;
    assert_eq!(settled["result"]["snapshot"]["goal"]["status"], json!("SUCCEEDED"), "{settled}");
    handle.shutdown().await.expect("shutdown");
}

/// The same rule for a plain reply: the first turn's answer must not be reported
/// as the answer to an input that was queued behind it. (Before D-72 the client
/// could report it whenever its poll fell inside the boundary between that turn's
/// reply and the drain that applies the queued input — a window of one driver
/// pass, which is why this test asserts the honest expectation rather than
/// reproducing the race.)
#[tokio::test]
async fn a_queued_input_is_not_answered_by_the_previous_turns_reply() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let first = json!({"__slow_ms__": 1200, "role": "assistant", "content": "answer to the first question"});
    let scripts = HashMap::from([("i-leader".to_string(), vec![first, reply("answer to the queued question")])]);
    let (root, handle) = boot("exec-queued-reply", scripts).await;
    let socket = root.dir.join("state/daemon.sock");
    let mut client = Client::connect(&socket).await;
    start_a_turn_in_flight(&mut client, "first question").await;
    let run = headless(exec_options(&socket, &root.dir.join("ws"), "second question", Vec::new())).await;
    assert_eq!(run.end, End::Reply, "{}", run.report);
    assert_eq!(run.report["reply"], json!("answer to the queued question"), "{}", run.report);
    handle.shutdown().await.expect("shutdown");
}

/// A queued input that a context reset seals never lands (§5.3/A24): the run has
/// nothing to wait for, so it says so at once instead of calling its own deadline
/// a timeout (D-72).
#[tokio::test]
async fn a_queued_input_a_reset_sealed_is_reported_undelivered() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let scripts = HashMap::from([(
        "i-leader".to_string(),
        vec![json!({"__slow_ms__": 4000, "role": "assistant", "content": "answer to the first question"})],
    )]);
    let (root, handle) = boot("exec-queued-sealed", scripts).await;
    let socket = root.dir.join("state/daemon.sock");
    let mut client = Client::connect(&socket).await;
    start_a_turn_in_flight(&mut client, "first question").await;
    // exec's input queues behind the slow turn; the user resets the instance,
    // which bumps the epoch, so the drain seals that envelope instead of applying it
    let exec_handle = tokio::task::spawn_blocking({
        let options =
            ExecOptions { timeout_s: 30, ..exec_options(&socket, &root.dir.join("ws"), "second question", Vec::new()) };
        move || execute(&options)
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    let reset = client.command("reset-1", "reset_instance", json!({"instance_id": "i-leader", "reason": "test"})).await;
    assert_eq!(reset["ok"], json!(true), "{reset}");
    let run = exec_handle.await.expect("exec task").expect("headless run");
    assert_eq!(run.end, End::Undelivered, "{}", run.report);
    assert_eq!(run.end.exit_code(run.checks_ok), 1);
    assert!(
        run.report["reply"] == Json::Null && run.report["goal_status"] == Json::Null,
        "an input that never landed answers nothing: {}",
        run.report
    );
    assert_eq!(run.report["verification"].as_array().unwrap().len(), 0, "there is no turn to accept: {}", run.report);
    handle.shutdown().await.expect("shutdown");
}

/// Nobody can answer an approval in a headless run: exec reports the parked
/// operation immediately (exit 3) instead of burning the deadline, and it does
/// not run the acceptance commands for a turn that never finished.
#[tokio::test]
async fn terminating_the_leader_mid_run_ends_the_headless_run_at_once() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    // the provider answers after three seconds: the run is in flight when the user retires the instance
    let slow = json!({"role": "assistant", "content": "late", "__slow_ms__": 3_000});
    let scripts = HashMap::from([("i-leader".to_string(), vec![slow])]);
    let (root, handle) = boot("exec-terminated", scripts).await;
    let (socket, workspace) = (root.dir.join("state/daemon.sock"), root.dir.join("ws"));
    let mut options = exec_options(&socket, &workspace, "write an essay", Vec::new());
    options.timeout_s = 30;
    let running = tokio::task::spawn_blocking(move || execute(&options));
    // wait until the leader is really waiting for the model, then terminate it as the user would
    let mut client = Client::connect(&socket).await;
    let mut in_flight = false;
    for _ in 0..400 {
        let checkpoint = client.call("checkpoint", json!({})).await;
        let instances = checkpoint["result"]["snapshot"]["instances"].clone();
        if instances.as_array().is_some_and(|rows| rows.iter().any(|row| row["phase"] == json!("MODEL_PENDING"))) {
            in_flight = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(in_flight, "the run never reached the model");
    client
        .command(
            "terminate-while-running",
            "set_lifecycle",
            json!({"instance_id": "i-leader", "lifecycle": "TERMINATED"}),
        )
        .await;
    let started = std::time::Instant::now();
    let run = running.await.expect("exec task").expect("headless run");
    // the run ends with the truth instead of waiting for its own deadline
    assert_eq!(run.end, End::Failed, "{}", run.report);
    assert_eq!(run.end.exit_code(run.checks_ok), 1);
    let failure = run.report["failure"].as_str().unwrap_or("");
    assert!(failure.contains("is terminated"), "{}", run.report);
    assert!(failure.contains("termination is final"), "the advice travels with it: {failure}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the termination ends the run at once ({}s)",
        started.elapsed().as_secs()
    );
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn pausing_the_leader_mid_run_lets_the_turn_finish() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    // the provider answers with a plain reply after three seconds: the pause arrives while the attempt is in
    // flight, and the reply still closes the turn (a pause stops the *instance*, at a boundary — D-98)
    let slow = json!({"role": "assistant", "content": "the work is done", "__slow_ms__": 3_000});
    let scripts = HashMap::from([("i-leader".to_string(), vec![slow])]);
    let (root, handle) = boot("exec-paused", scripts).await;
    let (socket, workspace) = (root.dir.join("state/daemon.sock"), root.dir.join("ws"));
    let mut options = exec_options(&socket, &workspace, "write an essay", Vec::new());
    options.timeout_s = 30;
    let running = tokio::task::spawn_blocking(move || execute(&options));
    let mut client = Client::connect(&socket).await;
    let mut in_flight = false;
    for _ in 0..400 {
        let checkpoint = client.call("checkpoint", json!({})).await;
        let instances = checkpoint["result"]["snapshot"]["instances"].clone();
        if instances.as_array().is_some_and(|rows| rows.iter().any(|row| row["phase"] == json!("MODEL_PENDING"))) {
            in_flight = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(in_flight, "the run never reached the model");
    client
        .command("pause-while-running", "set_lifecycle", json!({"instance_id": "i-leader", "lifecycle": "PAUSED"}))
        .await;
    let run = running.await.expect("exec task").expect("headless run");
    // the run reports the turn's own outcome, not a timeout and not the pause
    assert_eq!(run.end, End::Reply, "{}", run.report);
    assert_eq!(run.report["reply"], json!("the work is done"));
    assert_eq!(run.end.exit_code(run.checks_ok), 0);
    // ...and the instance really is paused: the lever did what it says
    let checkpoint = client.call("checkpoint", json!({})).await;
    let lifecycle = checkpoint["result"]["snapshot"]["instances"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["id"] == json!("i-leader")))
        .map(|row| row["lifecycle"].clone())
        .unwrap_or(Json::Null);
    assert_eq!(lifecycle, json!("PAUSED"), "{checkpoint}");
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn a_reset_mid_run_ends_the_headless_run_with_the_reason() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let slow = json!({"role": "assistant", "content": "late", "__slow_ms__": 3_000});
    let scripts = HashMap::from([("i-leader".to_string(), vec![slow])]);
    let (root, handle) = boot("exec-reset", scripts).await;
    let (socket, workspace) = (root.dir.join("state/daemon.sock"), root.dir.join("ws"));
    let mut options = exec_options(&socket, &workspace, "write an essay", Vec::new());
    options.timeout_s = 30;
    let running = tokio::task::spawn_blocking(move || execute(&options));
    let mut client = Client::connect(&socket).await;
    let mut in_flight = false;
    for _ in 0..400 {
        let checkpoint = client.call("checkpoint", json!({})).await;
        let instances = checkpoint["result"]["snapshot"]["instances"].clone();
        if instances.as_array().is_some_and(|rows| rows.iter().any(|row| row["phase"] == json!("MODEL_PENDING"))) {
            in_flight = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(in_flight, "the run never reached the model");
    client
        .command("reset-while-running", "reset_instance", json!({"instance_id": "i-leader", "reason": "probe reset"}))
        .await;
    let started = std::time::Instant::now();
    let run = running.await.expect("exec task").expect("headless run");
    assert_eq!(run.end, End::Failed, "{}", run.report);
    assert_eq!(run.end.exit_code(run.checks_ok), 1);
    let failure = run.report["failure"].as_str().unwrap_or("");
    assert!(failure.contains("was reset"), "{}", run.report);
    assert!(failure.contains("epoch 0 → 1"), "the epoch move travels with it: {failure}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "a reset ends the run at once ({}s)",
        started.elapsed().as_secs()
    );
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn a_budget_refusal_ends_the_headless_run_instead_of_timing_out() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let scripts = HashMap::from([("i-leader".to_string(), vec![finish_call("never asked")])]);
    let root = root("exec-budget-refused");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    // a ceiling below one request's estimate: the runtime refuses before any model call (A18)
    let mut cfg = config(&root, scripts);
    cfg.supervisor.goal_limits = json!({"max_total_tokens": 1});
    let handle = serve(cfg).await.expect("daemon");
    let (socket, workspace) = (root.dir.join("state/daemon.sock"), root.dir.join("ws"));
    let marker = workspace.join("check-ran");
    let mut options = exec_options(&socket, &workspace, "do something", vec![format!("touch {}", marker.display())]);
    options.timeout_s = 30;
    let started = std::time::Instant::now();
    let run = headless(options).await;
    // the refusal is this run's outcome: exit 1 with the runtime's reason, not "still running" (D-49's table)
    assert_eq!(run.end, End::Failed, "{}", run.report);
    assert_eq!(run.end.exit_code(run.checks_ok), 1);
    let failure = run.report["failure"].as_str().unwrap_or("");
    assert!(failure.contains("budget exceeded"), "{}", run.report);
    assert!(failure.contains("max 1"), "the reason carries the ceiling: {failure}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "a refused request ends the run at once, not at the deadline ({}s)",
        started.elapsed().as_secs()
    );
    // ...and a turn that never started has no acceptance to verify (D-96's rule, for a refusal)
    assert_eq!(run.report["verification"].as_array().unwrap().len(), 0, "{}", run.report);
    assert!(run.report["verification_path"].is_null(), "{}", run.report);
    assert!(!marker.exists(), "an acceptance command ran for a turn that never started");
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn a_run_that_times_out_verifies_nothing() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    // the provider answers just past the run's own deadline, so the turn is still in flight when the
    // caller gives up — and the daemon's own shutdown only waits for the attempt to land (1.5 s)
    let slow = json!({"role": "assistant", "content": "late", "__slow_ms__": 1_500});
    let scripts = HashMap::from([("i-leader".to_string(), vec![slow])]);
    let (root, handle) = boot("exec-timeout", scripts).await;
    let (socket, workspace) = (root.dir.join("state/daemon.sock"), root.dir.join("ws"));
    let marker = workspace.join("check-ran");
    let mut options = exec_options(&socket, &workspace, "never answered", vec![format!("touch {}", marker.display())]);
    options.timeout_s = 1;
    let started = std::time::Instant::now();
    let run = headless(options).await;
    assert_eq!(run.end, End::Timeout, "{}", run.report);
    assert_eq!(run.end.exit_code(run.checks_ok), 124);
    // an unfinished turn has no acceptance to verify, and a check must not stretch the deadline
    assert_eq!(run.report["verification"].as_array().unwrap().len(), 0, "{}", run.report);
    assert!(run.report["verification_path"].is_null(), "{}", run.report);
    assert!(!marker.exists(), "an unfinished run must not run the acceptance commands");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the caller's deadline bounds the run ({}s)",
        started.elapsed().as_secs()
    );
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn a_parked_approval_ends_the_headless_run_at_once() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let scripts = HashMap::from([("i-leader".to_string(), vec![shell_call("c1", "true")])]);
    let root = root("exec-approval");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let mut cfg = config(&root, scripts);
    cfg.supervisor.require_shell_approval = true;
    let handle = serve(cfg).await.expect("daemon");
    let (socket, workspace) = (root.dir.join("state/daemon.sock"), root.dir.join("ws"));
    let started = std::time::Instant::now();
    let run = headless(exec_options(&socket, &workspace, "run it", vec!["true".to_string()])).await;
    assert_eq!(run.end, End::ApprovalRequired, "{}", run.report);
    assert_eq!(run.end.exit_code(run.checks_ok), 3);
    assert!(run.report["approval"].as_str().is_some_and(|preview| !preview.is_empty()), "{}", run.report);
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "a parked approval must not wait for the deadline ({}s)",
        started.elapsed().as_secs()
    );
    assert_eq!(run.report["verification"].as_array().unwrap().len(), 0, "{}", run.report);
    handle.shutdown().await.expect("shutdown");
}

/// A daemon stop (`Ctrl-C`, `handle.shutdown`) must not wait out a provider's backoff: `Retry-After: 30 s` is a
/// real answer from a real service, and the supervisor awaits every driver task when it stops (D-117). Before the
/// retry wait became shutdown-aware this hung for the full 30 s with "stopping…" on screen.
#[tokio::test]
async fn a_daemon_stop_does_not_wait_out_a_provider_backoff() {
    let scripts = HashMap::from([(
        "i-leader".to_string(),
        vec![
            json!({"__transient__": "the service asked for 30 seconds", "__retry_after_ms__": 30_000}),
            reply("never"),
        ],
    )]);
    let (root, handle) = boot("stop-backoff", scripts).await;
    let socket = root.dir.join("state/daemon.sock");
    let mut client = Client::connect(&socket).await;
    let submitted = client.command("cmd-backoff", "submit_input", input_params("start a turn")).await;
    assert_eq!(submitted["ok"], json!(true), "{submitted}");
    // the first attempt failed and the driver is waiting for the retry; the attempt row is the record of that
    let mut seen = false;
    let waited = std::time::Instant::now();
    while waited.elapsed() < Duration::from_secs(10) {
        let events = client.call("events", json!({"since": 0})).await;
        if events["result"]["events"].as_array().is_some_and(|rows| {
            rows.iter().any(|event| {
                event["kind"] == json!("attempt_recorded") && event["payload"]["status"] == json!("FAILED")
            })
        }) {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(seen, "the failed attempt was recorded before the retry wait began");
    let stopping = std::time::Instant::now();
    handle.shutdown().await.expect("shutdown");
    let elapsed = stopping.elapsed();
    assert!(elapsed < Duration::from_secs(5), "the daemon stop waited out the provider backoff ({elapsed:?})");
}

/// D-248: the session's stop lever at the protocol level. `shutdown` is *answered* before the daemon goes — the
/// accept loop's flag is set only after the reply is on the wire — so the client that asked always has its
/// receipt and never a broken connection, and the socket leaves with the accept loop. The product surface on
/// top of this is `teamagents daemon --stop` (its end-to-end half is
/// `cli::daemon_stop_stops_the_session_by_its_socket`).
#[tokio::test]
async fn the_stop_lever_answers_before_the_daemon_goes() {
    let (root, handle) = boot("stop-lever", HashMap::new()).await;
    let socket = root.dir.join("state/daemon.sock");
    let mut client = Client::connect(&socket).await;
    let reply = client.call("shutdown", json!({})).await;
    assert_eq!(reply["ok"], json!(true), "{reply}");
    assert_eq!(reply["result"]["stopping"], json!(true), "{reply}");
    assert!(reply["result"]["state_root"].as_str().is_some_and(|path| path.ends_with("state")), "{reply}");
    // the reply above *is* the receipt; only after it was written does the accept loop leave, and the socket
    // goes with it
    for _ in 0..100 {
        if !socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(!socket.exists(), "the stopped daemon removes its socket: {socket:?}");
    handle.shutdown().await.expect("shutdown");
}

/// A scratch git repository with one commit, for the worktree tests (D-252).
fn init_git_repo(dir: &Path) {
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().expect("git");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "T"]);
    std::fs::write(dir.join("README.md"), "hi").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);
}

/// D-252: the worktree member's branch had no merge surface at all (`docs/ACCEPTANCE.md`: "nothing merges the
/// branch"), so the user merged by hand. `teamagents instances merge --id ID` brings it into the session's own
/// working tree, and refuses the three ways a merge would be wrong: a member in the middle of a turn, a checkout
/// with uncommitted work, and a member that is not a worktree at all.
#[tokio::test]
async fn the_instances_merge_lever_brings_a_worktree_members_branch_into_the_session_tree() {
    let root = root("merge-branch");
    let workspace = root.dir.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    init_git_repo(&workspace);
    // The leader spawns a worktree member, delegates to it and finishes its turn; the member's own turn is held
    // long enough for the test to try the merge *while it runs*.
    let leader = vec![
        json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": "spawn",
             "arguments": json!({"instance_id": "i-git", "instructions": "write the feature",
                                 "workspace": "git_worktree"}).to_string()}},
            {"id": "c2", "type": "function", "function": {"name": "delegate",
             "arguments": json!({"assignee": "i-git", "task_id": "t-feature", "description": "write it"}).to_string()}}
        ]}),
        finish_call("delegated"),
    ];
    let member = vec![
        json!({"role": "assistant", "content": "working", "__slow_ms__": 2000}),
        finish_call("the feature is written"),
    ];
    let scripts = HashMap::from([("i-leader".to_string(), leader), ("i-git".to_string(), member)]);
    let handle = serve(config(&root, scripts)).await.expect("daemon");
    let state = root.dir.join("state");
    let socket = state.join("daemon.sock");
    let mut client = Client::connect(&socket).await;
    let since = client.call("checkpoint", json!({})).await["result"]["watermark"].as_i64().unwrap();
    client.command("merge-input", "submit_input", input_params("delegate the feature")).await;
    let merge = |id: &str| {
        let (state, id) = (state.clone(), id.to_string());
        async move {
            tokio::task::spawn_blocking(move || {
                let out = std::process::Command::new(env!("CARGO_BIN_EXE_teamagents"))
                    .args(["instances", "merge", "--id", &id, "--json", "--state-root"])
                    .arg(&state)
                    .output()
                    .expect("run instances merge");
                (
                    out.status.code().unwrap_or(-1),
                    String::from_utf8_lossy(&out.stdout).into_owned(),
                    String::from_utf8_lossy(&out.stderr).into_owned(),
                )
            })
            .await
            .expect("join merge")
        }
    };
    // ... while the member works: the merge must refuse instead of racing its turn
    let worktree = state.join("instances/i-git/work");
    let mut busy = false;
    for _ in 0..400 {
        let checkpoint = client.call("checkpoint", json!({})).await;
        let phase = checkpoint["result"]["snapshot"]["instances"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["id"] == json!("i-git")))
            .and_then(|row| row["phase"].as_str())
            .unwrap_or("");
        // a turn in flight: the phases the driver reports while one is in progress
        if matches!(phase, "MODEL_PENDING" | "TOOLS_PENDING" | "WAITING") && worktree.join(".git").exists() {
            busy = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(busy, "the member's turn never went in flight");
    let (code, out, err) = merge("i-git").await;
    assert_eq!(code, 1, "a turn in flight is not a stable snapshot: {out}{err}");
    assert!(err.contains("middle of a turn"), "{err}");
    assert_eq!(wait_goal(&mut client, since).await.0, "SUCCEEDED");
    // the member's own turn ends on its own (a plain reply; D-65 leaves its task RUNNING, which does not block
    // merging what it has committed so far)
    let mut idle = false;
    for _ in 0..400 {
        let checkpoint = client.call("checkpoint", json!({})).await;
        let phase = checkpoint["result"]["snapshot"]["instances"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["id"] == json!("i-git")))
            .and_then(|row| row["phase"].as_str())
            .unwrap_or("");
        if phase == "READY" {
            idle = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(idle, "the member never went idle again");

    // the member committed its work in its own checkout; now the merge brings it into the session's tree
    std::fs::write(worktree.join("feature.txt"), "the feature\n").unwrap();
    for args in [vec!["add", "."], vec!["commit", "-q", "-m", "the feature"]] {
        let out = std::process::Command::new("git").arg("-C").arg(&worktree).args(&args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    let (code, out, err) = merge("i-git").await;
    assert_eq!(code, 0, "{out}{err}");
    let report: Json = serde_json::from_str(&out).expect("JSON report");
    assert_eq!(report["merged"], json!(true), "{report}");
    assert!(report["branch"].as_str().is_some_and(|branch| branch.starts_with("teamagents/")), "{report}");
    assert!(workspace.join("feature.txt").is_file(), "the member's file is in the session's tree now");
    let log =
        std::process::Command::new("git").arg("-C").arg(&workspace).args(["log", "--oneline", "-1"]).output().unwrap();
    assert!(String::from_utf8_lossy(&log.stdout).contains("merged by the user"), "{log:?}");

    // uncommitted work in the member's checkout is refused by name (the merge would leave it behind)
    std::fs::write(worktree.join("wip.txt"), "not committed\n").unwrap();
    let (code, _, err) = merge("i-git").await;
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("uncommitted changes"), "{err}");
    std::fs::remove_file(worktree.join("wip.txt")).unwrap();

    // and a member that is not a worktree has no branch to merge
    let (code, _, err) = merge("i-leader").await;
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("no branch to merge"), "{err}");
    handle.shutdown().await.expect("shutdown");
}

/// D-249: `exec --stream-json` prints the session's committed events as the run observes them — one typed line
/// each, in log order, never twice — and then the report as the last line, which is the same object `--json`
/// prints (the envelope does not change the report's shape). The streaming is a client of the `events(since)`
/// contract the daemon model pins (`verification/tla/V2Daemon.tla`: a client's view is the contiguous range
/// after its cursor, and the cursor never runs past the log), so the lines have to be a *prefix* of the log.
#[tokio::test]
async fn the_streaming_headless_mode_prints_the_events_then_the_report() {
    let scripts =
        HashMap::from([("i-leader".to_string(), vec![finish_call("streamed run"), finish_call("plain second run")])]);
    let (root, handle) = boot("exec-stream", scripts).await;
    let (state, workspace) = (root.dir.join("state"), root.dir.join("ws"));
    let cli = |args: Vec<String>| async move {
        tokio::task::spawn_blocking(move || {
            let output =
                std::process::Command::new(env!("CARGO_BIN_EXE_teamagents")).args(&args).output().expect("run exec");
            (
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stdout).into_owned(),
                String::from_utf8_lossy(&output.stderr).into_owned(),
            )
        })
        .await
        .expect("join exec")
    };
    let invoke = |mode: &str| {
        vec![
            "exec".to_string(),
            mode.to_string(),
            "--state-root".to_string(),
            state.to_string_lossy().into_owned(),
            "--cwd".to_string(),
            workspace.to_string_lossy().into_owned(),
            "--timeout".to_string(),
            "60".to_string(),
            "stream it".to_string(),
        ]
    };

    // 1. the streaming shape
    let (code, out, err) = cli(invoke("--stream-json")).await;
    assert_eq!(code, 0, "{out}{err}");
    let lines: Vec<Json> =
        out.lines().map(|line| serde_json::from_str(line).expect("every line is one JSON object")).collect();
    assert!(lines.len() >= 3, "at least the input, the settlement and the report: {out}");
    let (events, report_line) = lines.split_at(lines.len() - 1);
    assert_eq!(report_line[0]["type"], json!("report"), "{out}");
    let sequences: Vec<i64> = events
        .iter()
        .map(|line| {
            assert_eq!(line["type"], json!("event"), "every line before the report is an event: {out}");
            line["event"]["sequence"].as_i64().expect("a sequence")
        })
        .collect();
    assert!(sequences.windows(2).all(|pair| pair[1] > pair[0]), "the lines are the log in order: {sequences:?}");
    let report = &report_line[0]["report"];
    assert_eq!(report["end"], json!("completed"), "{report}");
    assert_eq!(
        report["watermark"],
        json!(*sequences.last().expect("at least one event")),
        "the report names the watermark the streamed lines reached: {report}"
    );

    // 2. the same run shape with `--json`: one line, and the report the stream's last line wrapped
    let (code, out, err) = cli(invoke("--json")).await;
    assert_eq!(out.lines().count(), 1, "--json prints one object and nothing else: {out}{err}");
    let plain: Json = serde_json::from_str(out.lines().next().unwrap()).expect("one JSON object");
    let mut streamed_keys: Vec<&str> = report.as_object().unwrap().keys().map(String::as_str).collect();
    let mut plain_keys: Vec<&str> = plain.as_object().unwrap().keys().map(String::as_str).collect();
    streamed_keys.sort_unstable();
    plain_keys.sort_unstable();
    assert_eq!(streamed_keys, plain_keys, "the streamed report is the `--json` report, in an envelope");
    assert_eq!(code, 1, "the second run has no active goal to settle, so it is the documented exit 1: {out}{err}");

    // 3. the two output shapes are refused together, with both flags named (D-230)
    let (code, out, err) = cli(vec![
        "exec".to_string(),
        "--json".to_string(),
        "--stream-json".to_string(),
        "--state-root".to_string(),
        state.to_string_lossy().into_owned(),
    ])
    .await;
    assert_eq!(code, 2, "a usage error, not a run: {out}{err}");
    assert!(err.contains("--json") && err.contains("--stream-json"), "{err}");
    handle.shutdown().await.expect("shutdown");
}
