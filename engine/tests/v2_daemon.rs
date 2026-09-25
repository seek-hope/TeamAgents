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
        // {"__error__": "…"} scripts a permanent provider failure
        if let Some(reason) = message["__error__"].as_str() {
            return Err(ProviderError::permanent(reason));
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

struct Root {
    dir: PathBuf,
}

fn root(tag: &str) -> Root {
    let dir = std::env::temp_dir().join(format!("teamagents-v2-daemon-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    Root { dir }
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
    assert!(out.contains("PAUSED"), "{out}");
    let (code, out, err) =
        cli(vec!["instances".into(), "resume".into(), "--id".into(), "i-worker".into()], state_root.clone()).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("ACTIVE"), "{out}");
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
#[tokio::test]
async fn headless_runs_verify_the_acceptance_commands_and_gate_the_exit_code() {
    if !teamagents_engine::tools::bwrap_available() {
        eprintln!("skipped: bwrap is unavailable");
        return;
    }
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let scripts = HashMap::from([("i-leader".to_string(), vec![finish_call("done")])]);
    let (root, handle) = boot("exec-checks", scripts).await;
    let (socket, workspace) = (root.dir.join("state/daemon.sock"), root.dir.join("ws"));
    let passed = headless(exec_options(&socket, &workspace, "finish it", vec!["echo checked".to_string()])).await;
    assert_eq!(passed.end, End::Completed, "{}", passed.report);
    assert!(passed.checks_ok, "{}", passed.report);
    assert_eq!(passed.end.exit_code(passed.checks_ok), 0);
    let verdicts = passed.report["verification"].as_array().unwrap();
    assert_eq!(verdicts.len(), 1, "{}", passed.report);
    assert_eq!(verdicts[0]["ok"], json!(true));
    assert_eq!(verdicts[0]["exit_code"], json!(0));
    assert!(verdicts[0]["output"].as_str().unwrap().contains("checked"), "{}", passed.report);
    let ledger = passed.report["verification_path"].as_str().expect("ledger path");
    assert_eq!(Path::new(ledger), root.dir.join("state/verification.json"));
    let written: Json = serde_json::from_str(&std::fs::read_to_string(ledger).unwrap()).unwrap();
    assert_eq!(written["verification"][0]["command"], json!("echo checked"));
    handle.shutdown().await.expect("shutdown");
}

/// A failing acceptance command turns a nominal success into exit 1 and stops
/// the remaining commands (later ones may depend on earlier ones).
#[tokio::test]
async fn a_failing_acceptance_command_fails_the_run() {
    if !teamagents_engine::tools::bwrap_available() {
        eprintln!("skipped: bwrap is unavailable");
        return;
    }
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
    assert_eq!(verdicts[0]["exit_code"], json!(7));
    assert!(verdicts[0]["output"].as_str().unwrap().contains("broken"), "{}", run.report);
    handle.shutdown().await.expect("shutdown");
}

/// Nobody can answer an approval in a headless run: exec reports the parked
/// operation immediately (exit 3) instead of burning the deadline, and it does
/// not run the acceptance commands for a turn that never finished.
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
