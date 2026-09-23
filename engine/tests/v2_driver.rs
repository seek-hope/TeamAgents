//! R2-P2 persistent single-instance driver end-to-end (scripted provider, no
//! real model): phase machine, crash recovery per §6.3, job reconnect (A10/
//! A11), tool-result reuse (A08), unknown outcomes (A09), cancellation,
//! approvals and budget parking. Every test uses a real session SQLite, real
//! runner processes and real marker files.

use serde_json::{json, Value as Json};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use teamagents_core::kernel::{KernelProfile, ModelRequest, ModelResponse, Usage};
use teamagents_core::models::UserConfig;
use teamagents_engine::providers::{AttemptOutcome, Cancel, Provider, ProviderError, ProviderEvent};
use teamagents_engine::v2::driver::{start, DriverConfig, DriverHandle};

enum Step {
    Message(Json),
    /// Block until the driver task is aborted (in-flight crash simulation).
    Hang,
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
        let next = self.script.lock().unwrap().pop_front().unwrap_or(Step::Message(reply("script exhausted")));
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
