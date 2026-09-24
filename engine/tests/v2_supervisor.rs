//! R2-P3 supervisor end-to-end (scripted providers, no real model): one
//! coordinator discovers and drives every ACTIVE instance; spawned workers
//! join mid-run; collaboration (spawn/delegate/wait/settle) executes through
//! the control plane; termination retires drivers and lets the loop exit.

use serde_json::{json, Value as Json};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use teamagents_core::kernel::{KernelProfile, ModelRequest, ModelResponse, Usage};
use teamagents_core::models::{ModelProfile, UserConfig};
use teamagents_engine::providers::{AttemptOutcome, Cancel, Provider, ProviderError, ProviderEvent};
use teamagents_engine::v2::supervisor::{start, SupervisorConfig, SupervisorHandle};

enum Step {
    Message(Json),
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
        }
    }
}

fn reply(text: &str) -> Json {
    json!({"role": "assistant", "content": text})
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
    let dir = std::env::temp_dir().join(format!("teamagents-v2-supervisor-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    Root { dir }
}

/// Scripts dispatched by instance id; an unscripted instance just replies.
fn factory(scripts: HashMap<String, Vec<Step>>) -> impl Fn(&str, &KernelProfile) -> ScriptedProvider {
    let scripts = Mutex::new(scripts);
    move |id: &str, _profile: &KernelProfile| {
        let script = scripts.lock().unwrap().remove(id).unwrap_or_default();
        ScriptedProvider { script: Mutex::new(script.into()) }
    }
}

fn config<P, F>(root: &Root, provider_factory: F) -> SupervisorConfig<P, F>
where
    F: Fn(&str, &KernelProfile) -> P,
{
    SupervisorConfig {
        marker: std::marker::PhantomData,
        session_db: root.dir.join("session.sqlite"),
        session_id: "s-test".into(),
        leader_id: "i-leader".into(),
        leader_profile: KernelProfile {
            model: "scripted".into(),
            instructions: "team leader".into(),
            tools: vec![json!({"type": "function", "function": {
                "name": "shell", "description": "run a shell command",
                "parameters": {"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}
            }})],
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
        provider_factory,
    }
}

fn cmd(id: &str, method: &str, params: Json) -> teamagents_core::v2::Command {
    teamagents_core::v2::Command { command_id: id.into(), method: method.into(), params }
}

/// Second control connection over the same session DB (WAL multi-connection).
fn second_control(root: &Root) -> teamagents_core::v2::Control {
    teamagents_core::v2::Control::open(&root.dir.join("session.sqlite"), "s-test", false).expect("control")
}

async fn wait_instance_phase(handle: &SupervisorHandle, instance: &str, phase: &str, timeout_ms: u64) {
    for _ in 0..(timeout_ms / 25) {
        if let Ok(snapshot) = handle.snapshot().await {
            if snapshot["instances"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["id"] == json!(instance) && i["phase"] == json!(phase))
            {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("instance {instance} did not reach phase {phase} within {timeout_ms}ms");
}

async fn wait_event(handle: &SupervisorHandle, kind: &str, timeout_ms: u64) -> Json {
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

#[tokio::test]
async fn spawned_worker_settles_and_the_leader_completes() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("spawn-loop");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    // leader: spawn the worker, delegate with an explicit task id, wait on
    // the task, finish. worker: one finish settles the delegated task.
    let leader = vec![
        Step::Message(json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": "spawn",
             "arguments": json!({"instance_id": "i-worker", "instructions": "math helper"}).to_string()}},
            {"id": "c2", "type": "function", "function": {"name": "delegate",
             "arguments": json!({"assignee": "i-worker", "task_id": "t-answer", "description": "what is 2+2"}).to_string()}}
        ]})),
        Step::Message(wait_call("c3", json!({"mode": "ANY", "conditions": [{"kind": "task", "task_id": "t-answer"}]}))),
        Step::Message(finish_call("team answered 4")),
    ];
    let worker = vec![Step::Message(finish_call("4"))];
    let handle = start(config(
        &root,
        factory(HashMap::from([("i-leader".to_string(), leader), ("i-worker".to_string(), worker)])),
    ))
    .await
    .expect("start");
    // the leader may manage the session (spawn authority, §5.1)
    let mut control = second_control(&root);
    control
        .submit(
            cmd(
                "g-manage",
                "issue_grant",
                json!({"subject": "i-leader", "action": "manage", "resource_scope": "session"}),
            ),
            teamagents_core::v2::Identity::User,
        )
        .expect("manage grant");
    handle.input("i-leader", "get me 2+2 via a worker").await.expect("input");
    let closed = wait_event(&handle, "goal_completed", 20_000).await;
    assert_eq!(closed["payload"]["status"], json!("SUCCEEDED"));
    // the spawned worker joined, ran and settled the delegated task
    let task: String =
        control.connection().query_row("SELECT status FROM tasks WHERE id = 't-answer'", [], |row| row.get(0)).unwrap();
    assert_eq!(task, "SUCCEEDED");
    let snapshot = handle.snapshot().await.unwrap();
    let ids: Vec<&str> = snapshot["instances"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"i-leader") && ids.contains(&"i-worker"), "{ids:?}");
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn a_waiting_leader_wakes_when_the_peers_message_is_applied() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("wait-wake");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    // peer: greet the leader, then close its own empty turn. leader: park on
    // the peer's message, finish when it lands.
    let peer = vec![Step::Message(send_call("c1", "i-leader", "hello leader")), Step::Message(finish_call("greeted"))];
    let leader = vec![
        Step::Message(wait_call("c1", json!({"mode": "ANY", "conditions": [{"kind": "message", "from": "i-peer"}]}))),
        Step::Message(finish_call("got it")),
    ];
    // the peer exists before the coordinator starts; discovery picks it up
    let handle =
        start(config(&root, factory(HashMap::from([("i-leader".to_string(), leader), ("i-peer".to_string(), peer)]))))
            .await
            .expect("start");
    let mut control = second_control(&root);
    control
        .submit(
            cmd(
                "mk-peer",
                "create_instance",
                json!({"id": "i-peer", "workspace_ref": root.dir.join("ws").to_string_lossy()}),
            ),
            teamagents_core::v2::Identity::User,
        )
        .expect("peer");
    control
        .submit(
            cmd(
                "g-msg",
                "issue_grant",
                json!({"subject": "i-peer", "action": "message", "resource_scope": "instance:i-leader"}),
            ),
            teamagents_core::v2::Identity::User,
        )
        .expect("message grant");
    handle.input("i-leader", "wait for the peer").await.expect("leader input");
    // deterministic parked path (A23): the message arrives only after the
    // leader is WAITING, so its own parked drain is the only wake path
    wait_instance_phase(&handle, "i-leader", "WAITING", 10_000).await;
    handle.input("i-peer", "greet the leader").await.expect("peer input");
    let closed = wait_event(&handle, "goal_completed", 20_000).await;
    assert_eq!(closed["payload"]["status"], json!("SUCCEEDED"));
    // the message was applied to the leader's context, not left queued
    let applied: i64 = control
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM context_entries WHERE instance_id = 'i-leader' AND message_json LIKE '%hello leader%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(applied, 1, "the parked drain must apply the envelope (A23)");
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn terminating_every_instance_lets_the_supervisor_exit() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("retire");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let handle = start(config(&root, factory(HashMap::new()))).await.expect("start");
    let snapshot = handle.snapshot().await.unwrap();
    assert_eq!(snapshot["instances"].as_array().unwrap().len(), 1);
    handle.set_lifecycle("i-leader", "TERMINATED").await.expect("terminate");
    // the driver retires and the discovery loop exits: shutdown must not hang
    tokio::time::timeout(Duration::from_secs(10), handle.shutdown())
        .await
        .expect("supervisor did not exit after all instances terminated")
        .expect("shutdown");
}

#[tokio::test]
async fn shutdown_with_an_active_instance_still_exits() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("shutdown-active");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let handle = start(config(&root, factory(HashMap::new()))).await.expect("start");
    let snapshot = handle.snapshot().await.unwrap();
    assert_eq!(snapshot["instances"].as_array().unwrap().len(), 1);
    // the leader stays ACTIVE: shutdown must stop the discovery loop instead
    // of respawning drivers for it
    tokio::time::timeout(Duration::from_secs(10), handle.shutdown())
        .await
        .expect("supervisor did not exit with an active instance")
        .expect("shutdown");
}

/// Minimal SSE-replay HTTP server (one raw response per request; records
/// request bodies) for wire-level heterogeneous tests.
struct FakeServer {
    base: String,
    bodies: std::sync::Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl FakeServer {
    async fn start(responses: Vec<String>) -> FakeServer {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let bodies = std::sync::Arc::new(Mutex::new(vec![]));
        let seen = bodies.clone();
        let task = tokio::spawn(async move {
            for response in responses {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let mut buf = vec![0u8; 65536];
                let mut head = Vec::new();
                loop {
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    head.extend_from_slice(&buf[..n]);
                    if let Some(pos) = head.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&head[..pos]).to_string();
                        let content_length: usize = headers
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length: ").or_else(|| line.strip_prefix("Content-Length: "))
                            })
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        let mut body = head[pos + 4..].to_vec();
                        while body.len() < content_length {
                            let n = socket.read(&mut buf).await.unwrap_or(0);
                            if n == 0 {
                                break;
                            }
                            body.extend_from_slice(&buf[..n]);
                        }
                        seen.lock().unwrap().push(String::from_utf8_lossy(&body).to_string());
                        break;
                    }
                }
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            }
        });
        FakeServer { base: format!("http://{addr}"), bodies, task }
    }

    fn bodies(&self) -> Vec<String> {
        self.bodies.lock().unwrap().clone()
    }
}

fn sse_response(events: &str) -> String {
    format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{events}")
}

/// Poll until one model request of the instance reached COMPLETE (proof the
/// provider round-trip went through the real wire adapter).
async fn wait_model_request_complete(control: &teamagents_core::v2::Control, instance: &str) {
    for _ in 0..400 {
        let count: i64 = control
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM model_requests WHERE instance_id = ?1 AND status = 'COMPLETE'",
                [instance],
                |row| row.get(0),
            )
            .unwrap();
        if count > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no COMPLETE model request for {instance}");
}

#[tokio::test]
async fn heterogeneous_instances_run_different_protocols_in_one_session() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("hetero");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    // Leader speaks chat completions, the worker speaks Responses: per-model
    // catalog routing by protocol, never by vendor (R17, pi-ai style).
    let chat_server = FakeServer::start(vec![sse_response(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"f1\",\"function\":{\"name\":\"finish\",\"arguments\":\"{\\\"status\\\":\\\"success\\\",\\\"summary\\\":\\\"lead done\\\"}\"}}]}}]}\n\n\
         data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n\
         data: [DONE]\n\n",
    )])
    .await;
    let responses_server = FakeServer::start(vec![sse_response(
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"function_call\",\"call_id\":\"f1\",\"name\":\"finish\",\"arguments\":\"{\\\"status\\\":\\\"success\\\",\\\"summary\\\":\\\"worker done\\\"}\",\"status\":\"completed\"}],\"usage\":{\"input_tokens\":3,\"output_tokens\":2,\"total_tokens\":5}}}\n\n",
    )])
    .await;
    let mut catalog = UserConfig::default();
    catalog.models.insert(
        "lead-model".into(),
        ModelProfile {
            provider: "deepseek".into(),
            protocol: "deepseek".into(),
            model: "ds-flash".into(),
            base_url: Some(chat_server.base.clone()),
            api_key_env: None,
            timeout: 5,
            max_retries: 0,
            generation_options: Default::default(),
            context_window: Some(1_000_000),
            codex_profile: None,
        },
    );
    catalog.models.insert(
        "worker-model".into(),
        ModelProfile {
            provider: "openai".into(),
            protocol: "responses".into(),
            model: "gpt-x".into(),
            base_url: Some(responses_server.base.clone()),
            api_key_env: None,
            timeout: 5,
            max_retries: 0,
            generation_options: Default::default(),
            context_window: Some(200_000),
            codex_profile: None,
        },
    );
    let catalog = std::sync::Arc::new(catalog);
    let build = catalog.clone();
    let factory = move |_id: &str, profile: &KernelProfile| {
        teamagents_engine::providers::build_for_model(&build, &profile.model).expect("provider")
    };
    let mut cfg = config(&root, factory);
    cfg.leader_profile.model = "lead-model".into();
    cfg.catalog = (*catalog).clone();
    let handle = start(cfg).await.expect("start");
    let mut control = second_control(&root);
    control
        .submit(
            cmd(
                "mk-worker",
                "create_instance",
                json!({"id": "i-worker", "workspace_ref": root.dir.join("ws").to_string_lossy(),
                       "profile": {"model": "worker-model"}}),
            ),
            teamagents_core::v2::Identity::User,
        )
        .expect("worker");
    wait_instance_phase(&handle, "i-worker", "READY", 10_000).await;
    handle.input("i-leader", "close your turn").await.expect("leader input");
    handle.input("i-worker", "close your turn").await.expect("worker input");
    // both instances completed one model request through their own protocol
    wait_model_request_complete(&control, "i-leader").await;
    wait_model_request_complete(&control, "i-worker").await;
    let chat = chat_server.bodies();
    assert_eq!(chat.len(), 1, "exactly one leader request: {chat:?}");
    assert!(chat[0].contains("\"model\":\"ds-flash\"") && chat[0].contains("\"messages\""), "{}", chat[0]);
    let responded = responses_server.bodies();
    assert_eq!(responded.len(), 1, "exactly one worker request: {responded:?}");
    assert!(responded[0].contains("\"model\":\"gpt-x\"") && responded[0].contains("\"input\""), "{}", responded[0]);
    assert!(responded[0].contains("\"store\":false"), "{}", responded[0]);
    handle.shutdown().await.expect("shutdown");
    chat_server.task.await.unwrap();
    responses_server.task.await.unwrap();
}

/// §12.3 wiring: a terminated instance's isolated workspace is retired by the
/// supervisor (and its record dropped), so nothing accumulates silently.
#[tokio::test]
async fn terminating_an_instance_retires_its_workspace() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("retire-workspace");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let leader = vec![
        Step::Message(json!({"role": "assistant", "content": "", "tool_calls": [{"id": "c1", "type": "function",
            "function": {"name": "spawn", "arguments": json!({"instance_id": "i-iso", "instructions": "helper",
                                                              "workspace": "isolated"}).to_string()}}]})),
        Step::Message(finish_call("spawned an isolated helper")),
    ];
    let worker = vec![Step::Message(reply("idle"))];
    let handle = start(config(
        &root,
        factory(HashMap::from([("i-leader".to_string(), leader), ("i-worker".to_string(), worker)])),
    ))
    .await
    .expect("start");
    let mut control = second_control(&root);
    control
        .submit(
            cmd(
                "g-manage",
                "issue_grant",
                json!({"subject": "i-leader", "action": "manage", "resource_scope": "session"}),
            ),
            teamagents_core::v2::Identity::User,
        )
        .expect("manage grant");
    handle.input("i-leader", "spawn an isolated helper").await.expect("input");

    let workspace = root.dir.join("state/instances/i-iso/work");
    let record = root.dir.join("state/instances/i-iso/workspace.json");
    for _ in 0..(10_000 / 25) {
        if workspace.join("INPUTS.md").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(workspace.join("INPUTS.md").exists(), "the isolated workspace was created");
    assert!(record.exists(), "and its policy was recorded");

    handle.set_lifecycle("i-iso", "TERMINATED").await.expect("terminate");
    let mut retired = false;
    for _ in 0..(10_000 / 25) {
        if !workspace.exists() && !record.exists() {
            retired = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(retired, "the supervisor retired the isolated workspace and its record");
    assert!(root.dir.join("ws").exists(), "the shared project directory is untouched");
    handle.shutdown().await.expect("shutdown");
}
