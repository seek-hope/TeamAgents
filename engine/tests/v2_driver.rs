//! R2-P2 persistent single-instance driver end-to-end (scripted provider, no
//! real model): phase machine, crash recovery per §6.3, job reconnect (A10/
//! A11), tool-result reuse (A08), unknown outcomes (A09), cancellation,
//! approvals and budget parking. Every test uses a real session SQLite, real
//! runner processes and real marker files.

use serde_json::{json, Value as Json};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use teamagents_core::kernel::{KernelProfile, ModelRequest, ModelResponse, Usage};
use teamagents_core::models::UserConfig;
use teamagents_engine::providers::{AttemptOutcome, Cancel, Provider, ProviderError, ProviderEvent};
use teamagents_engine::v2::driver::{start, DriverConfig, DriverHandle};

enum Step {
    Message(Json),
    /// Block until the driver task is aborted (in-flight crash simulation).
    Hang,
    /// Sleep, then continue with the next step: a slow provider call.
    Sleep(u64),
    /// Fail the attempt the way the provider edge reports a permanent error.
    Error(&'static str),
}

struct ScriptedProvider {
    script: Mutex<VecDeque<Step>>,
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
        let mut next = self.script.lock().unwrap().pop_front().unwrap_or(Step::Message(reply("script exhausted")));
        while let Step::Sleep(ms) = next {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            next = self.script.lock().unwrap().pop_front().unwrap_or(Step::Message(reply("script exhausted")));
        }
        match next {
            Step::Message(message) => Ok(AttemptOutcome {
                response: ModelResponse {
                    message,
                    usage: Some(Usage { prompt: 3, completion: 2, total: 5 }),
                    native: json!({}),
                },
                raw: json!({"scripted": true}),
                elapsed_ms: 1,
            }),
            Step::Hang => loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
            },
            Step::Sleep(_) => unreachable!("sleeps are consumed above"),
            Step::Error(message) => Err(ProviderError::permanent(message)),
        }
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

fn wait_call(id: &str, wait: Json) -> Json {
    json!({"role": "assistant", "content": "",
           "tool_calls": [{"id": id, "type": "function",
                           "function": {"name": "wait", "arguments": wait.to_string()}}]})
}

fn send_call(id: &str, recipient: &str, text: &str) -> Json {
    json!({"role": "assistant", "content": "",
           "tool_calls": [{"id": id, "type": "function",
                           "function": {"name": "send",
                                        "arguments": json!({"recipient": recipient, "text": text}).to_string()}}]})
}

struct Root {
    dir: PathBuf,
}

fn root(tag: &str) -> Root {
    let dir = std::env::temp_dir().join(format!("teamagents-v2-driver-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    Root { dir }
}

impl Root {
    fn config(&self, provider: ScriptedProvider) -> DriverConfig<ScriptedProvider> {
        self.config_with(provider)
    }

    fn config_with<P: Provider + 'static>(&self, provider: P) -> DriverConfig<P> {
        DriverConfig {
            session_db: self.dir.join("session.sqlite"),
            session_id: "s-test".into(),
            instance_id: "i-main".into(),
            state_root: self.dir.join("state"),
            workspace: self.dir.join("ws"),
            permissions: "full_auto".into(),
            profile: KernelProfile {
                model: "scripted".into(),
                instructions: "test agent".into(),
                tools: vec![json!({"type": "function", "function": {
                    "name": "shell", "description": "run a shell command",
                    "parameters": {"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}
                }})],
                options: json!({}),
                context_window: Some(128_000),
            },
            provider,
            catalog: UserConfig::default(),
            bindings: vec![],
            max_retries: 2,
            storage_queue: 64,
            poll: Duration::from_millis(15),
            goal_limits: json!({}),
            require_shell_approval: false,
        }
    }
}

async fn wait_event(handle: &DriverHandle, kind: &str, timeout_ms: u64) -> Json {
    for _ in 0..(timeout_ms / 25) {
        if let Ok(events) = handle.events(0).await {
            if let Some(event) = events.iter().find(|e| e["kind"] == json!(kind)) {
                return event.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("event {kind} did not arrive within {timeout_ms}ms");
}

async fn wait_phase(handle: &DriverHandle, phase: &str, timeout_ms: u64) {
    for _ in 0..(timeout_ms / 25) {
        if let Ok(snapshot) = handle.snapshot().await {
            if snapshot["instance"]["phase"] == json!(phase) {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("phase {phase} not reached within {timeout_ms}ms");
}

async fn run_to_goal_close(handle: &DriverHandle) -> String {
    let event = wait_event(handle, "goal_completed", 15_000).await;
    event["payload"]["status"].as_str().unwrap_or("").to_string()
}

/// Scripted provider that also records every request it was asked to send, so
/// a test can prove what the model actually saw (§7, A20). The log is shared
/// because the driver takes ownership of the provider.
type RequestLog = Arc<Mutex<Vec<Json>>>;

struct RecordingProvider {
    inner: ScriptedProvider,
    seen: RequestLog,
}

fn recording(script: Vec<Step>) -> (RecordingProvider, RequestLog) {
    let seen: RequestLog = Arc::new(Mutex::new(Vec::new()));
    (RecordingProvider { inner: ScriptedProvider { script: Mutex::new(script.into()) }, seen: seen.clone() }, seen)
}

fn recorded(log: &RequestLog) -> Vec<Json> {
    log.lock().unwrap().clone()
}

/// Every message text of one recorded request, joined: assertions about what
/// the model saw read from this, never from the persisted context.
fn request_text(request: &Json) -> String {
    request["messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .map(|message| {
                    let mut text = message["content"].as_str().unwrap_or("").to_string();
                    if let Some(calls) = message["tool_calls"].as_array() {
                        for call in calls {
                            text.push_str(call["function"]["name"].as_str().unwrap_or(""));
                        }
                    }
                    text
                })
                .collect::<Vec<String>>()
                .join("\n")
        })
        .unwrap_or_default()
}

impl Provider for RecordingProvider {
    fn protocol(&self) -> &str {
        self.inner.protocol()
    }
    async fn complete(
        &self,
        request: &ModelRequest,
        cancel: &Cancel,
        on_event: &mut (dyn FnMut(ProviderEvent) + Send),
    ) -> Result<AttemptOutcome, ProviderError> {
        self.seen.lock().unwrap().push(json!({
            "messages": request.messages,
            "tools": request.tools.len(),
            "est_prompt_tokens": request.est_prompt_tokens,
        }));
        self.inner.complete(request, cancel, on_event).await
    }
}

/// `read_history` is the built-in readback tool: it pages the stored output of
/// an earlier call, covered entries included (A20).
fn readback_call(id: &str, source: &str) -> Json {
    json!({"role": "assistant", "content": "",
           "tool_calls": [{"id": id, "type": "function",
                           "function": {"name": "read_history",
                                        "arguments": json!({"tool_call_id": source}).to_string()}}]})
}

/// Wait until the provider has been asked at least `count` times.
async fn wait_for_requests(log: &RequestLog, count: usize, timeout_ms: u64) -> Vec<Json> {
    for _ in 0..(timeout_ms / 25) {
        let requests = recorded(log);
        if requests.len() >= count {
            return requests;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{count} provider request(s) did not arrive within {timeout_ms}ms");
}

/// Wait until at least `count` events of one kind have arrived.
async fn wait_event_count(handle: &DriverHandle, kind: &str, count: usize, timeout_ms: u64) -> Vec<Json> {
    for _ in 0..(timeout_ms / 25) {
        if let Ok(events) = handle.events(0).await {
            let matching: Vec<Json> = events.into_iter().filter(|event| event["kind"] == json!(kind)).collect();
            if matching.len() >= count {
                return matching;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{count} {kind} event(s) did not arrive within {timeout_ms}ms");
}

fn cmd(id: &str, method: &str, params: Json) -> teamagents_core::v2::Command {
    teamagents_core::v2::Command { command_id: id.into(), method: method.into(), params }
}

/// Second control connection over the same session DB (WAL multi-connection).
fn second_control(root: &Root) -> teamagents_core::v2::Control {
    teamagents_core::v2::Control::open(&root.dir.join("session.sqlite"), "s-test", true).expect("control")
}

/// Wait for one task's settlement event.
async fn wait_task_settled(handle: &DriverHandle, task_id: &str, timeout_ms: u64) -> Json {
    for _ in 0..(timeout_ms / 25) {
        if let Ok(events) = handle.events(0).await {
            if let Some(event) = events
                .iter()
                .find(|e| e["kind"] == json!("task_completed") && e["payload"]["task_id"] == json!(task_id))
            {
                return event.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("task {task_id} not settled within {timeout_ms}ms");
}

/// Create an instance with no goal — the worker shape (§5.3): the driver
/// bootstrap skips existing instances, so no goal is attached and
/// completions settle tasks instead of goals.
fn make_worker(control: &mut teamagents_core::v2::Control, root: &Root, instance: &str) {
    control
        .submit(
            cmd(
                &format!("make-{instance}"),
                "create_instance",
                json!({"id": instance, "workspace_ref": root.dir.join("ws").to_string_lossy()}),
            ),
            teamagents_core::v2::Identity::User,
        )
        .expect("worker instance");
}

/// Start the driver for a pre-created (goal-less) worker instance.
async fn start_worker(root: &Root, instance: &str, script: Vec<Step>) -> DriverHandle {
    let mut config = root.config(ScriptedProvider { script: Mutex::new(script.into()) });
    config.instance_id = instance.into();
    start(config).await.expect("worker driver")
}

#[tokio::test]
async fn end_to_end_shell_then_finish() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("e2e");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let marker = root.dir.join("ws").join("marker.txt");
    let script = vec![
        Step::Message(shell_call("c1", &format!("echo v2-works > {}", marker.display()))),
        Step::Message(finish_call("wrote the marker")),
    ];
    let handle = start(root.config(ScriptedProvider { script: Mutex::new(script.into()) })).await.expect("start");
    handle.input("write the marker").await.expect("input");
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), "v2-works");
    let snapshot = handle.snapshot().await.unwrap();
    assert_eq!(snapshot["instance"]["phase"], json!("READY"));
    assert_eq!(snapshot["goal"]["known_usage"]["total"], json!(10)); // two attempts billed
                                                                     // context: user input, assistant tool call, tool result, assistant finish
    let events = handle.events(0).await.unwrap();
    assert!(events.iter().any(|e| e["kind"] == json!("response_imported")));
    assert!(events.iter().any(|e| e["kind"] == json!("operation_completed")));
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn wait_parks_and_the_timer_wake_lets_the_model_continue() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("wait-timer");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    // the model waits on a message that never comes, with a 1s deadline;
    // after the timer wake it finishes (A23/§5.3 end to end)
    let script = vec![
        Step::Message(wait_call(
            "w1",
            json!({"mode": "ANY", "conditions": [{"kind": "message", "from": "i-peer"}], "timer_seconds": 1}),
        )),
        Step::Message(finish_call("waited, nothing came, done")),
    ];
    let handle = start(root.config(ScriptedProvider { script: Mutex::new(script.into()) })).await.expect("start");
    handle.input("try waiting").await.expect("input");
    wait_phase(&handle, "WAITING", 5_000).await;
    // the driver fires due timers at poll granularity; the wake reason joins
    // the context and the model is prompted again
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    let events = handle.events(0).await.unwrap();
    assert!(events.iter().any(|e| e["kind"] == json!("wait_satisfied")), "{events:?}");
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn send_executes_through_the_control_plane_with_a_grant() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("collab-send");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let script = vec![Step::Message(send_call("c1", "i-peer", "hello peer")), Step::Message(finish_call("sent"))];
    let handle = start(root.config(ScriptedProvider { script: Mutex::new(script.into()) })).await.expect("start");
    // a second connection issues the peer instance and the message grant
    let mut control =
        teamagents_core::v2::Control::open(&root.dir.join("session.sqlite"), "s-test", false).expect("open");
    control
        .submit(
            teamagents_core::v2::Command {
                command_id: "test-peer".into(),
                method: "create_instance".into(),
                params: json!({"id": "i-peer", "workspace_ref": ""}),
            },
            teamagents_core::v2::Identity::User,
        )
        .expect("peer");
    control
        .submit(
            teamagents_core::v2::Command {
                command_id: "test-grant".into(),
                method: "issue_grant".into(),
                params: json!({"subject": "i-main", "action": "message", "resource_scope": "instance:i-peer"}),
            },
            teamagents_core::v2::Identity::User,
        )
        .expect("grant");
    handle.input("say hello").await.expect("input");
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    // the envelope persists for the peer's boundary (§5.3)
    let queued: i64 = control
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM envelopes WHERE recipient = 'i-peer' AND kind = 'message' AND payload_json LIKE '%hello peer%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(queued, 1);
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn crash_before_model_response_recovers_with_honest_billing() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("crash-model");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    // first driver: the provider hangs mid-attempt; the task is aborted
    let hanging = ScriptedProvider { script: Mutex::new(vec![Step::Hang].into()) };
    let handle = start(root.config(hanging)).await.expect("start");
    handle.input("do work").await.expect("input");
    wait_phase(&handle, "MODEL_PENDING", 5_000).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    handle.crash();
    // recovery: the lost attempt is recorded with unknown usage, then the
    // request retries under the same request and completes (§6.3)
    let recovering = ScriptedProvider { script: Mutex::new(vec![Step::Message(finish_call("recovered"))].into()) };
    let handle = start(root.config(recovering)).await.expect("restart");
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    let snapshot = handle.snapshot().await.unwrap();
    assert_eq!(snapshot["goal"]["unknown_usage"], json!(1), "the lost attempt must stay visible");
    assert_eq!(snapshot["goal"]["known_usage"]["total"], json!(5), "exactly one complete attempt billed");
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn tool_result_is_reused_after_crash_not_reexecuted() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("crash-tool");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let marker = root.dir.join("ws").join("runs.txt");
    // the shell job appends a line; re-execution would be visible as a dup
    let script = vec![
        Step::Message(shell_call("c1", &format!("echo ran >> {}", marker.display()))),
        Step::Hang, // never answer again: the crash hits after the receipt
    ];
    let handle = start(root.config(ScriptedProvider { script: Mutex::new(script.into()) })).await.expect("start");
    handle.input("run once").await.expect("input");
    // wait until the terminal receipt is committed (A08 crash window)
    wait_event(&handle, "operation_completed", 10_000).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.crash();
    let recovering = ScriptedProvider { script: Mutex::new(vec![Step::Message(finish_call("consumed"))].into()) };
    let handle = start(root.config(recovering)).await.expect("restart");
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    let runs = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(runs.matches("ran").count(), 1, "the committed receipt must be reused, not re-executed");
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn runner_crash_mid_job_is_outcome_unknown() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("crash-runner");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let script = vec![
        Step::Message(shell_call("c1", "sleep 300")),
        Step::Message(reply("the shell outcome is unknown, I will not guess")),
    ];
    let handle = start(root.config(ScriptedProvider { script: Mutex::new(script.into()) })).await.expect("start");
    handle.input("run a long command").await.expect("input");
    // wait for the job to be RUNNING, then kill the runner process itself
    let job_dir = root.dir.join("state").join("jobs");
    let mut killed = false;
    for _ in 0..200 {
        tokio::time::sleep(Duration::from_millis(25)).await;
        let Ok(entries) = std::fs::read_dir(&job_dir) else { continue };
        let Some(entry) = entries.flatten().next() else { continue };
        let journal_path = entry.path().join("journal.json");
        let Ok(text) = std::fs::read_to_string(&journal_path) else { continue };
        if !text.contains("\"RUNNING\"") {
            continue;
        }
        let output = std::process::Command::new("pgrep")
            .args(["-f", &format!("jobs-runner.*{}", entry.path().display())])
            .output()
            .expect("pgrep");
        let pids = String::from_utf8_lossy(&output.stdout);
        for pid in pids.lines() {
            std::process::Command::new("kill").args(["-KILL", pid]).output().expect("kill");
            killed = true;
        }
        if killed {
            break;
        }
    }
    assert!(killed, "runner process not found/killed");
    // the driver must surface OUTCOME_UNKNOWN instead of guessing (A09)
    let event = wait_event(&handle, "operation_completed", 15_000).await;
    assert_eq!(event["payload"]["status"], json!("OUTCOME_UNKNOWN"));
    wait_event(&handle, "response_imported", 10_000).await;
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn user_cancel_stops_a_running_job() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("cancel");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let script = vec![Step::Message(shell_call("c1", "sleep 300")), Step::Message(reply("cancelled, moving on"))];
    let handle = start(root.config(script.into_provider())).await.expect("start");
    handle.input("run something long").await.expect("input");
    wait_phase(&handle, "TOOLS_PENDING", 10_000).await;
    tokio::time::sleep(Duration::from_millis(300)).await; // let the job start
    let cancelled = handle.cancel_operation("d-stub:0").await;
    // the real operation id is derived from the request id; find it via events
    let events = handle.events(0).await.unwrap();
    let op = events
        .iter()
        .find(|e| e["kind"] == json!("operation_dispatched"))
        .map(|e| e["payload"]["operation_id"].as_str().unwrap_or("").to_string())
        .expect("dispatched op");
    if cancelled.is_err() {
        handle.cancel_operation(&op).await.expect("cancel op");
    }
    let event = wait_event(&handle, "operation_completed", 10_000).await;
    assert_eq!(event["payload"]["status"], json!("CANCELLED"));
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn budget_exhaustion_parks_the_instance() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("budget");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let mut config = root.config(ScriptedProvider { script: Mutex::new(vec![].into()) });
    config.goal_limits = json!({"max_total_tokens": 1});
    let handle = start(config).await.expect("start");
    handle.input("impossible within budget").await.expect("input");
    wait_event(&handle, "budget_refused", 5_000).await;
    wait_phase(&handle, "READY", 5_000).await;
    let snapshot = handle.snapshot().await.unwrap();
    assert_eq!(snapshot["instance"]["lifecycle"], json!("PARKED"));
    handle.shutdown().await.expect("shutdown");
}

/// A35: the goal deadline is enforced by the daemon even if the eval
/// client died — past it, new requests are refused and the instance parks
/// with the reason instead of silently continuing to answer.
#[tokio::test]
async fn goal_deadline_parks_the_instance() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("deadline");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let config = root.config(ScriptedProvider { script: Mutex::new(vec![].into()) });
    let db = config.session_db.clone();
    let handle = start(config).await.expect("start");
    {
        let conn = rusqlite::Connection::open(&db).expect("open db");
        conn.execute("UPDATE goals SET deadline = ?1", [teamagents_core::models::now() - 1.0]).expect("deadline");
    }
    handle.input("answer after the deadline").await.expect("input");
    wait_event(&handle, "goal_deadline_refused", 5_000).await;
    let snapshot = handle.snapshot().await.unwrap();
    assert_eq!(snapshot["instance"]["lifecycle"], json!("PARKED"));
    handle.shutdown().await.expect("shutdown");
}

/// A31/§4.4: a full disk stops new side-effect dispatch, the in-flight
/// persistence loss is reported via the park reason, and a user resume
/// completes the work exactly once — the lost write never double-bills.
#[tokio::test]
async fn disk_full_stops_dispatch_reports_and_resumes_after_parking() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("full");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    // the bulky assistant entry forces real page allocation at import time
    let bulk = "y".repeat(200_000);
    let big_finish = |id: &str, summary: &str| {
        json!({"role": "assistant", "content": bulk,
               "tool_calls": [{"id": id, "type": "function",
                               "function": {"name": "finish",
                                            "arguments": json!({"status": "success", "summary": summary}).to_string()}}]})
    };
    let script = vec![
        Step::Sleep(400),
        Step::Message(big_finish("f1", "first")),
        // only consumed when the first attempt could not be recorded at all
        Step::Message(big_finish("f2", "second")),
    ];
    let config = root.config(ScriptedProvider { script: Mutex::new(script.into()) });
    let handle = start(config).await.expect("start");
    wait_phase(&handle, "READY", 5_000).await;
    handle.input("finish with a bulky reply").await.expect("input");
    // begin_request commits before the provider call; the sleep leaves a wide
    // window to cap the database before the import write
    let mut began = false;
    for _ in 0..80 {
        let count = handle
            .with_control(|control| {
                control
                    .connection()
                    .query_row("SELECT COUNT(*) FROM model_requests", [], |row| row.get::<_, i64>(0))
                    .map_err(|e| e.to_string())
            })
            .await
            .expect("storage")
            .expect("request count");
        if count == 1 {
            began = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(began, "begin_request did not commit");
    // repack, then cap the shared storage connection at its current size:
    // the import's allocation fails with a genuine SQLITE_FULL (A31)
    let pages = handle
        .with_control(|control| {
            control.connection().execute_batch("VACUUM").map_err(|e| e.to_string())?;
            control
                .connection()
                .pragma_query_value(None, "page_count", |row| row.get::<_, i64>(0))
                .map_err(|e| e.to_string())
        })
        .await
        .expect("storage")
        .expect("vacuum and page count");
    handle
        .with_control(move |control| {
            control.connection().pragma_update(None, "max_page_count", pages).map_err(|e| e.to_string())
        })
        .await
        .expect("storage")
        .expect("cap pages");
    // the provider sleep must be fully behind us before judging the latch
    tokio::time::sleep(Duration::from_millis(700)).await;
    // while the disk stays full the bulky import can never land: no
    // completion, no finish dispatch, the request stays open, and the driver
    // keeps breathing. (The small park write itself may fit in-page slack and
    // park immediately — the desired report — or wait for space; both are
    // correct §4.4 outcomes.)
    for _ in 0..12 {
        let snapshot = handle.snapshot().await.expect("snapshot");
        assert_eq!(snapshot["instance"]["phase"], json!("MODEL_PENDING"));
        let events = handle.events(0).await.expect("events");
        assert!(!events.iter().any(|e| e["kind"] == json!("goal_completed")), "no completion may land while full");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    // free space: the parked instance can finish the interrupted write again
    handle
        .with_control(|control| {
            control.connection().pragma_update(None, "max_page_count", 1_073_741_823i64).map_err(|e| e.to_string())
        })
        .await
        .expect("storage")
        .expect("uncap pages");
    let mut parked_reason = String::new();
    for _ in 0..200 {
        if let Ok(events) = handle.events(0).await {
            if let Some(event) = events
                .iter()
                .find(|e| e["kind"] == json!("instance_lifecycle") && e["payload"]["lifecycle"] == json!("PARKED"))
            {
                parked_reason = event["payload"]["reason"].as_str().unwrap_or("").to_string();
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(parked_reason.contains("storage full"), "the park reason reports the loss: {parked_reason}");
    // the user resumes: the stored response imports (or the attempt re-runs
    // exactly once) and the goal completes exactly once
    handle.resume().await.expect("resume");
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    let attempts = handle
        .with_control(|control| {
            control
                .connection()
                .query_row("SELECT COUNT(*) FROM attempts", [], |row| row.get::<_, i64>(0))
                .map_err(|e| e.to_string())
        })
        .await
        .expect("storage")
        .expect("attempt count");
    assert_eq!(attempts, 1, "the in-flight loss never double-bills");
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn shell_approval_blocks_then_allows_and_denial_cancels() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("approve");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let approved_marker = root.dir.join("ws").join("approved.txt");
    let denied_marker = root.dir.join("ws").join("denied.txt");
    let script = vec![
        Step::Message(shell_call("c1", &format!("touch {}", approved_marker.display()))),
        Step::Message(shell_call("c2", &format!("touch {}", denied_marker.display()))),
        Step::Message(finish_call("approval flow done")),
    ];
    let mut config = root.config(ScriptedProvider { script: Mutex::new(script.into()) });
    config.require_shell_approval = true;
    let handle = start(config).await.expect("start");
    handle.input("two approved-scope commands").await.expect("input");
    // first shell: approval required → approve → runs
    let asked = wait_event(&handle, "approval_requested", 10_000).await;
    let approval_id = asked["payload"]["approval_id"].as_str().unwrap().to_string();
    assert!(!approved_marker.exists());
    handle.approve(&approval_id).await.expect("approve");
    for _ in 0..200 {
        if approved_marker.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(approved_marker.exists(), "approved command must run");
    // second shell: deny → the operation is cancelled before any execution
    let asked = wait_for_second_approval(&handle).await;
    handle.deny(&asked).await.expect("deny");
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    assert!(!denied_marker.exists(), "a denied command never runs");
    handle.shutdown().await.expect("shutdown");
}

async fn wait_for_second_approval(handle: &DriverHandle) -> String {
    for _ in 0..400 {
        let events = handle.events(0).await.unwrap();
        let asks: Vec<&Json> = events.iter().filter(|e| e["kind"] == json!("approval_requested")).collect();
        if asks.len() >= 2 {
            return asks[1]["payload"]["approval_id"].as_str().unwrap().to_string();
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("second approval request did not arrive");
}

trait IntoProvider {
    fn into_provider(self) -> ScriptedProvider;
}

impl IntoProvider for Vec<Step> {
    fn into_provider(self) -> ScriptedProvider {
        ScriptedProvider { script: Mutex::new(self.into()) }
    }
}

#[tokio::test]
async fn worker_finish_settles_the_delegated_task_from_the_stored_candidate() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("worker-settle");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    // goal + delegation land before the worker driver starts (§5.3)
    let mut control = second_control(&root);
    make_worker(&mut control, &root, "i-worker");
    control.submit(cmd("mk-g", "create_goal", json!({"id": "g1"})), teamagents_core::v2::Identity::User).expect("goal");
    control
        .submit(
            cmd("dt-1", "delegate_task", json!({"task_id": "t1", "assignee": "i-worker", "goal_id": "g1"})),
            teamagents_core::v2::Identity::User,
        )
        .expect("delegate");
    let script = vec![Step::Message(finish_call("task one done"))];
    let handle = start_worker(&root, "i-worker", script).await;
    let settled = wait_task_settled(&handle, "t1", 15_000).await;
    assert_eq!(settled["payload"]["status"], json!("SUCCEEDED"));
    assert_eq!(settled["payload"]["assignee"], json!("i-worker"));
    // the task carries the stored candidate's outcome; the worker adopted
    // it (PENDING → RUNNING) without any model tool call
    let row: (String, String) = control
        .connection()
        .query_row("SELECT status, result_refs_json FROM tasks WHERE id = 't1'", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(row.0, "SUCCEEDED");
    assert_eq!(row.1, "[]", "no evidence in the candidate means no result refs");
    wait_phase(&handle, "READY", 5_000).await;
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn worker_queue_continues_after_a_settlement_until_empty() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("worker-queue");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let mut control = second_control(&root);
    make_worker(&mut control, &root, "i-worker");
    control.submit(cmd("mk-g", "create_goal", json!({"id": "g1"})), teamagents_core::v2::Identity::User).expect("goal");
    for (tag, task) in [("dt-1", "t1"), ("dt-2", "t2")] {
        control
            .submit(
                cmd(tag, "delegate_task", json!({"task_id": task, "assignee": "i-worker", "goal_id": "g1"})),
                teamagents_core::v2::Identity::User,
            )
            .expect("delegate");
    }
    // one finish per task: settling t1 must not park the worker — the queue
    // continuation note wakes the next turn, which adopts and settles t2
    let script = vec![Step::Message(finish_call("first done")), Step::Message(finish_call("second done"))];
    let handle = start_worker(&root, "i-worker", script).await;
    wait_task_settled(&handle, "t1", 15_000).await;
    wait_task_settled(&handle, "t2", 15_000).await;
    let statuses: Vec<String> = control
        .connection()
        .prepare("SELECT status FROM tasks WHERE id IN ('t1', 't2') ORDER BY rowid")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(statuses, vec!["SUCCEEDED", "SUCCEEDED"]);
    // the continuation note joined the context between the two turns
    let notes: i64 = control
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM context_entries WHERE instance_id = 'i-worker' AND kind = 'note'
             AND message_json LIKE '%settled: SUCCEEDED%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(notes, 1, "exactly one settlement note (t1) — the queue was empty after t2");
    wait_phase(&handle, "READY", 5_000).await;
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn worker_finish_with_an_empty_queue_just_closes_the_turn() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("worker-idle");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let mut control = second_control(&root);
    make_worker(&mut control, &root, "i-worker");
    let script = vec![Step::Message(finish_call("nothing to settle"))];
    let handle = start_worker(&root, "i-worker", script).await;
    handle.input("hello worker").await.expect("input");
    // no goal, no tasks: the finish closes without an error (§5.2)
    wait_event(&handle, "completion_closed", 15_000).await;
    wait_phase(&handle, "READY", 5_000).await;
    handle.shutdown().await.expect("shutdown");
}

fn finish_call_blocked(summary: &str) -> Json {
    json!({"role": "assistant", "content": "",
           "tool_calls": [{"id": "finish-1", "type": "function",
                           "function": {"name": "finish",
                                        "arguments": json!({"status": "blocked", "summary": summary}).to_string()}}]})
}

async fn wait_event_where(kind: &str, handle: &DriverHandle, timeout_ms: u64, pred: impl Fn(&Json) -> bool) -> Json {
    for _ in 0..(timeout_ms / 25) {
        if let Ok(events) = handle.events(0).await {
            if let Some(event) = events.iter().find(|e| e["kind"] == json!(kind) && pred(e)) {
                return event.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("event {kind} did not arrive within {timeout_ms}ms");
}

async fn no_event(handle: &DriverHandle, kind: &str) -> bool {
    !handle.events(0).await.unwrap().iter().any(|e| e["kind"] == json!(kind))
}

#[tokio::test]
async fn required_checks_pass_settles_the_goal() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("checks-pass");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let script = vec![Step::Message(finish_call("all done")), Step::Message(finish_call("all done again"))];
    let mut config = root.config(ScriptedProvider { script: Mutex::new(script.into()) });
    config.goal_limits = json!({"required_checks": [{"id": "tests", "command": "true"}]});
    let handle = start(config).await.expect("start");
    handle.input("do the work").await.expect("input");
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    // a second finish after the goal closed must settle, not park the
    // instance in COMPLETION_PENDING (§8): no turn starts after the close
    tokio::time::sleep(Duration::from_millis(300)).await;
    let settled = handle.events(0).await.unwrap();
    let closed = settled.iter().position(|e| e["kind"] == json!("goal_completed")).expect("goal_completed");
    assert!(
        !settled[closed + 1..].iter().any(|e| e["kind"] == json!("request_began")),
        "no turn may start after the goal closed: {settled:?}"
    );
    assert_eq!(handle.snapshot().await.unwrap()["instance"]["phase"], json!("READY"));
    // the check rode the same operation ledger and auto-associated (§8)
    let events = handle.events(0).await.unwrap();
    let registered = events.iter().find(|e| e["kind"] == json!("check_round_registered")).expect("registered");
    assert_eq!(registered["payload"]["round"], json!(1));
    assert!(events.iter().any(|e| e["kind"] == json!("operation_completed")
        && e["scope"].as_str().unwrap_or("").starts_with("check:goal-s-test:1:")));
    let snapshot = handle.snapshot().await.unwrap();
    assert_eq!(snapshot["instance"]["phase"], json!("READY"));
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn required_checks_failure_repairs_then_passes() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("checks-repair");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    // round 1: finish, the check fails (marker missing); the repair turn
    // creates the marker and finishes again; round 2 passes (§8)
    let script = vec![
        Step::Message(finish_call("claimed done")),
        Step::Message(shell_call("fix-1", "touch done-marker")),
        Step::Message(finish_call("actually done now")),
    ];
    let mut config = root.config(ScriptedProvider { script: Mutex::new(script.into()) });
    config.goal_limits = json!({"required_checks": [{"id": "marker", "command": "test -f done-marker"}]});
    let handle = start(config).await.expect("start");
    handle.input("do the work").await.expect("input");
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    let repair = wait_event_where("completion_repair", &handle, 5_000, |_| true).await;
    assert_eq!(repair["payload"]["round"], json!(1));
    assert_eq!(repair["payload"]["failures"][0]["check_id"], json!("marker"));
    assert_eq!(repair["payload"]["failures"][0]["class"], json!("exit"));
    let events = handle.events(0).await.unwrap();
    let rounds: Vec<_> = events.iter().filter(|e| e["kind"] == json!("check_round_registered")).collect();
    assert_eq!(rounds.len(), 2);
    assert!(root.dir.join("ws").join("done-marker").exists());
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn required_checks_exhausted_parks_the_goal_blocked() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("checks-exhaust");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let script = vec![Step::Message(finish_call("try one")), Step::Message(finish_call("try two"))];
    let mut config = root.config(ScriptedProvider { script: Mutex::new(script.into()) });
    config.goal_limits = json!({"required_checks": [{"id": "never", "command": "exit 1"}], "max_check_rounds": 2});
    let handle = start(config).await.expect("start");
    handle.input("do the work").await.expect("input");
    let blocked = wait_event_where("goal_blocked", &handle, 20_000, |_| true).await;
    assert_eq!(blocked["payload"]["status"], json!("BLOCKED"));
    let reason = blocked["payload"]["reason"].as_str().unwrap_or("");
    assert!(reason.contains("never:exit"), "{reason}");
    let snapshot = handle.snapshot().await.unwrap();
    assert_eq!(snapshot["goal"]["status"], json!("BLOCKED"));
    assert_eq!(snapshot["instance"]["phase"], json!("READY"));
    assert!(no_event(&handle, "goal_completed").await);
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn blocked_candidate_never_runs_the_checks() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("checks-blocked");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    // §8: a candidate that admits undelivered work is never upgraded by
    // passing checks — the checks do not even run
    let script = vec![Step::Message(finish_call_blocked("could not finish the migration"))];
    let mut config = root.config(ScriptedProvider { script: Mutex::new(script.into()) });
    config.goal_limits = json!({"required_checks": [{"id": "tests", "command": "true"}]});
    let handle = start(config).await.expect("start");
    handle.input("migrate").await.expect("input");
    let closed =
        wait_event_where("goal_completed", &handle, 15_000, |e| e["payload"]["status"] == json!("BLOCKED")).await;
    assert_eq!(closed["payload"]["completion"]["outcome"], json!("blocked"));
    assert!(no_event(&handle, "check_round_registered").await);
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn check_inputs_must_still_hold_at_completion() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("checks-stale");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    std::fs::write(root.dir.join("ws").join("input.txt"), "original").unwrap();
    // the check itself mutates its declared input: even with exit 0 the
    // result bound nothing (§8 hash re-verification before completion)
    let script = vec![Step::Message(finish_call("done"))];
    let mut config = root.config(ScriptedProvider { script: Mutex::new(script.into()) });
    config.goal_limits = json!({"required_checks": [{"id": "bound", "command": "printf changed > input.txt",
                                                     "inputs": ["input.txt"]}],
                                "max_check_rounds": 1});
    let handle = start(config).await.expect("start");
    handle.input("do the work").await.expect("input");
    let blocked = wait_event_where("goal_blocked", &handle, 20_000, |_| true).await;
    assert!(blocked["payload"]["reason"].as_str().unwrap_or("").contains("bound:stale_inputs"));
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn check_dispatch_refused_parks_without_burning_repair_rounds() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("checks-refused");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    // an instance with no shell@workspace grant cannot run checks; that is
    // verification-infrastructure failure, not model-repairable work (§8)
    let mut control = second_control(&root);
    control
        .submit(
            cmd("mk-i", "create_instance", json!({"id": "i-main", "workspace_ref": ""})),
            teamagents_core::v2::Identity::User,
        )
        .expect("instance");
    control
        .submit(
            cmd(
                "mk-g",
                "create_goal",
                json!({"id": "goal-s-test", "instance_id": "i-main",
                                              "limits": {"required_checks": [{"id": "tests", "command": "true"}]}}),
            ),
            teamagents_core::v2::Identity::User,
        )
        .expect("goal");
    drop(control);
    let script = vec![Step::Message(finish_call("claimed done"))];
    let handle = start(root.config(ScriptedProvider { script: Mutex::new(script.into()) })).await.expect("start");
    handle.input("do the work").await.expect("input");
    let blocked = wait_event_where("goal_blocked", &handle, 20_000, |_| true).await;
    assert!(blocked["payload"]["reason"].as_str().unwrap_or("").contains("dispatch_refused"));
    assert!(no_event(&handle, "completion_repair").await);
    handle.shutdown().await.expect("shutdown");
}

/// R22/A20: a context that no longer fits the native window is compacted
/// before the turn is fixed. The summary call is billed to the goal, the
/// covered text stays reachable through readback, and a restarted driver
/// keeps using the compacted view.
#[tokio::test]
async fn long_context_compacts_before_the_turn_and_survives_a_restart() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("compact");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    // 32k chars ≈ 8k estimated tokens: over 90% of an 8k native window, but
    // far under it once the summary replaces the covered entries
    let bulk = format!("the long specification {}", "padding ".repeat(4_000));
    let summary = "[Compacted conversation summary]\n1. Original request: keep the evidence";
    let script = vec![
        Step::Message(shell_call("c1", "echo stored-evidence")),
        Step::Message(reply("first answer")),
        Step::Message(reply(summary)),
        Step::Message(readback_call("c2", "c1")),
        Step::Message(reply("second answer")),
    ];
    let (provider, log) = recording(script);
    let mut config = root.config_with(provider);
    config.profile.context_window = Some(8_000);
    let handle = start(config).await.expect("start");
    handle.input(&bulk).await.expect("input");
    // turn one runs uncompressed: no assistant response exists to summarize
    wait_event_count(&handle, "response_imported", 2, 15_000).await;
    assert_eq!(recorded(&log).len(), 2, "turn one must not summarize yet");
    // turn two arrives with the long input still in view: compact first
    handle.input("now continue").await.expect("input");
    let compressed = wait_event_count(&handle, "context_compressed", 1, 15_000).await;
    assert_eq!(compressed[0]["payload"]["covered"], json!(4), "everything before the new input is covered");
    assert_eq!(compressed[0]["payload"]["kept"], json!(1), "the new input itself stays verbatim");
    wait_event_count(&handle, "response_imported", 4, 15_000).await;
    let requests = recorded(&log);
    assert_eq!(requests.len(), 5, "one summary call plus two attempts per turn");
    // the summary call is a plain prompt: no system message, no tools (§7)
    let compaction = &requests[2];
    assert_eq!(compaction["tools"], json!(0));
    assert_eq!(compaction["messages"][0]["role"], json!("user"));
    assert!(request_text(compaction).contains("You are compacting an agent conversation"));
    assert!(request_text(compaction).contains("the long specification"));
    // the turn that follows sees the summary, never the covered original
    let compacted_turn = request_text(&requests[3]);
    assert!(compacted_turn.contains("Compacted conversation summary"), "{compacted_turn}");
    assert!(!compacted_turn.contains("the long specification"), "covered text must leave the view");
    assert!(compacted_turn.contains("now continue"), "the newest input stays verbatim");
    // the compression call is billed to the same goal (A18/A20): 5 attempts
    let snapshot = handle.snapshot().await.unwrap();
    assert_eq!(snapshot["goal"]["known_usage"]["total"], json!(25));
    let mut control = second_control(&root);
    let (kind, status): (String, String) = control
        .connection()
        .query_row(
            "SELECT kind, status FROM model_requests WHERE kind = 'compression' ORDER BY rowid LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("compression request");
    assert_eq!((kind.as_str(), status.as_str()), ("compression", "COMPLETE"));
    // readback still reaches the covered tool output (A20 原文可追溯)
    let text: String = control
        .connection()
        .query_row(
            "SELECT message_json FROM context_entries WHERE instance_id = 'i-main' AND kind = 'tool_result'
             ORDER BY idx DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("readback result");
    assert!(text.contains("stored-evidence"), "{text}");
    // and the originals stay in the epoch for the user-facing history (A05)
    let history = control
        .submit(cmd("rh", "read_history", json!({"instance_id": "i-main"})), teamagents_core::v2::Identity::User)
        .expect("history");
    let entries = history["entries"].as_array().cloned().unwrap_or_default();
    assert!(
        entries
            .iter()
            .any(|entry| entry["message"]["content"].as_str().unwrap_or("").contains("the long specification")),
        "the covered original must remain readable"
    );
    handle.shutdown().await.expect("shutdown");

    // restart over the same session: the compacted view is the persisted state
    let (restarted, restarted_log) = recording(vec![Step::Message(reply("third answer"))]);
    let mut config = root.config_with(restarted);
    config.profile.context_window = Some(8_000);
    let handle = start(config).await.expect("restart");
    handle.input("after the restart").await.expect("input");
    // events survive the restart, so wait on the new driver's own request log
    let requests = wait_for_requests(&restarted_log, 1, 15_000).await;
    assert_eq!(requests.len(), 1, "a small view needs no further summary");
    let text = request_text(&requests[0]);
    assert!(text.contains("Compacted conversation summary"), "{text}");
    assert!(!text.contains("the long specification"), "the restart keeps the compacted view");
    handle.shutdown().await.expect("shutdown");
}

/// R22/A20: a failed summary is a lost optimization, never a failed turn —
/// the turn proceeds uncompressed, the reservation is released, and three
/// consecutive failures stop the attempts (D-28 breaker).
#[tokio::test]
async fn failed_summaries_fall_back_uncompressed_and_break_after_three() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("compact-fail");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let bulk = format!("the long specification {}", "padding ".repeat(4_000));
    let script = vec![
        Step::Message(reply("first answer")),
        Step::Error("summarizer unavailable"),
        Step::Message(reply("second answer")),
        Step::Error("summarizer unavailable"),
        Step::Message(reply("third answer")),
        Step::Error("summarizer unavailable"),
        Step::Message(reply("fourth answer")),
        Step::Message(reply("fifth answer")),
    ];
    let (provider, log) = recording(script);
    let mut config = root.config_with(provider);
    config.profile.context_window = Some(8_000);
    let handle = start(config).await.expect("start");
    handle.input(&bulk).await.expect("input");
    wait_event_count(&handle, "response_imported", 1, 15_000).await;
    for round in 1..=3 {
        handle.input(&format!("round {round}")).await.expect("input");
        wait_event_count(&handle, "compression_failed", round, 15_000).await;
        wait_event_count(&handle, "response_imported", round + 1, 15_000).await;
    }
    // the breaker stops trying: the fourth input goes straight to the model
    handle.input("round 4").await.expect("input");
    wait_event_count(&handle, "response_imported", 5, 15_000).await;
    let events = handle.events(0).await.unwrap();
    let count = |kind: &str| events.iter().filter(|event| event["kind"] == json!(kind)).count();
    assert_eq!(count("compression_began"), 3);
    assert_eq!(count("compression_failed"), 3);
    assert_eq!(count("context_compressed"), 0);
    // every turn after the failures still ran, uncompressed and unbilled for
    // the failed summaries: the last request is an ordinary turn request
    let requests = recorded(&log);
    assert_eq!(requests.len(), 8, "4 turns + 3 failed summaries + 1 breaker-skipped turn");
    let last = requests.last().unwrap();
    assert_eq!(last["messages"][0]["role"], json!("system"));
    assert!(request_text(last).contains("the long specification"), "the view stays uncompressed");
    // the failures released their reservations and closed their requests
    let control = second_control(&root);
    let reservations: String = control
        .connection()
        .query_row("SELECT reservations_json FROM goals LIMIT 1", [], |row| row.get(0))
        .expect("goal");
    assert_eq!(reservations, "{}");
    let open: i64 = control
        .connection()
        .query_row("SELECT COUNT(*) FROM model_requests WHERE kind = 'compression'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(open, 3, "exactly the three attempted summaries are recorded");
    handle.shutdown().await.expect("shutdown");
}
