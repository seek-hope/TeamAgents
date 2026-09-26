//! R2-P3 supervisor end-to-end (scripted providers, no real model): one
//! coordinator discovers and drives every ACTIVE instance; spawned workers
//! join mid-run; collaboration (spawn/delegate/wait/settle) executes through
//! the control plane; termination retires drivers and lets the loop exit.

use serde_json::{json, Value as Json};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use teamagents_core::kernel::{KernelProfile, ModelRequest, ModelResponse, Usage};
use teamagents_core::models::{ModelProfile, UserConfig};
use teamagents_engine::providers::{AttemptOutcome, Cancel, Provider, ProviderError, ProviderEvent};
use teamagents_engine::v2::supervisor::{start, SupervisorConfig, SupervisorHandle};

enum Step {
    Message(Json),
    /// Reply after a delay, so a test can act *while* the request is in flight
    /// (the harness logs the request before the delay).
    Slow(u64, Json),
}

type Seen = Arc<Mutex<HashMap<String, Vec<Vec<String>>>>>;

struct ScriptedProvider {
    instance: String,
    script: Mutex<VecDeque<Step>>,
    seen: Seen,
}

impl Provider for ScriptedProvider {
    fn protocol(&self) -> &str {
        "scripted"
    }
    async fn complete(
        &self,
        request: &ModelRequest,
        _cancel: &Cancel,
        _on_event: &mut (dyn FnMut(ProviderEvent) + Send),
    ) -> Result<AttemptOutcome, ProviderError> {
        self.seen.lock().unwrap().entry(self.instance.clone()).or_default().push(
            request.tools.iter().filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string)).collect(),
        );
        let next = self.script.lock().unwrap().pop_front().unwrap_or(Step::Message(reply("script exhausted")));
        let (message, delay) = match next {
            Step::Message(message) => (message, 0),
            Step::Slow(ms, message) => (message, ms),
        };
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        match Step::Message(message) {
            Step::Message(message) => Ok(AttemptOutcome {
                response: ModelResponse {
                    message,
                    usage: Some(Usage { prompt: 3, completion: 2, total: 5 }),
                    native: json!({}),
                },
                raw: json!({"scripted": true}),
                elapsed_ms: 1,
            }),
            Step::Slow(..) => unreachable!("the delay was already applied"),
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
    factory_with_log(scripts, Arc::new(Mutex::new(HashMap::new())))
}

/// The same factory, keeping every request's tool surface per instance so a test
/// can assert what a model was *offered* (the spec-to-code correspondence of
/// V2Grants' `OfferedToolsAreAuthorized`, D-60).
fn factory_with_log(
    scripts: HashMap<String, Vec<Step>>,
    seen: Seen,
) -> impl Fn(&str, &KernelProfile) -> ScriptedProvider {
    let scripts = Mutex::new(scripts);
    move |id: &str, _profile: &KernelProfile| {
        let script = scripts.lock().unwrap().remove(id).unwrap_or_default();
        ScriptedProvider { instance: id.to_string(), script: Mutex::new(script.into()), seen: seen.clone() }
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

/// The spec-to-code correspondence of `V2Grants.tla`'s
/// `OfferedToolsAreAuthorized`: the model-visible surface never offers a tool the
/// instance cannot dispatch.
///
/// The leader holds `shell@workspace` from the session bootstrap, so it is offered
/// `shell` (and its team authority). A child it spawns holds no shell grant
/// (§5.1), so `shell` must not appear in the child's surface either — before D-60
/// the profile handed it out and every call came back as "holds no
/// shell@workspace grant".
#[tokio::test]
async fn the_offered_surface_follows_the_grants() {
    let leader = vec![
        Step::Message(json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": "spawn",
             "arguments": json!({"instance_id": "i-worker", "instructions": "file helper"}).to_string()}},
            {"id": "c2", "type": "function", "function": {"name": "delegate",
             "arguments": json!({"assignee": "i-worker", "task_id": "t-1",
                                 "description": "write out/answer.txt"}).to_string()}}
        ]})),
        Step::Message(wait_call("c3", json!({"mode": "ANY", "conditions": [{"kind": "task", "task_id": "t-1"}]}))),
        Step::Message(finish_call("delegated")),
    ];
    let worker = vec![Step::Message(finish_call("written"))];
    let seen: Seen = Arc::new(Mutex::new(HashMap::new()));
    let root = root("offered-surface");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let handle = start(config(
        &root,
        factory_with_log(
            HashMap::from([("i-leader".to_string(), leader), ("i-worker".to_string(), worker)]),
            seen.clone(),
        ),
    ))
    .await
    .expect("start");
    // no grant is issued by hand: the session bootstrap authorizes its leader (D-58)
    handle.input("i-leader", "delegate the file to a worker").await.expect("input");
    let closed = wait_event(&handle, "goal_completed", 20_000).await;
    assert_eq!(closed["payload"]["status"], json!("SUCCEEDED"), "{closed}");
    handle.shutdown().await.expect("shutdown");

    let seen = seen.lock().unwrap().clone();
    let offered = |id: &str| seen.get(id).and_then(|requests| requests.first()).cloned().unwrap_or_default();
    let leader = offered("i-leader");
    assert!(leader.contains(&"shell".to_string()), "the leader holds shell@workspace: {leader:?}");
    assert!(leader.contains(&"spawn".to_string()), "and its team authority: {leader:?}");
    let child = offered("i-worker");
    assert!(
        !child.contains(&"shell".to_string()),
        "a spawned child holds no shell grant, so it must not be offered shell: {child:?}"
    );
    assert!(
        child.contains(&"finish".to_string()) && child.contains(&"wait".to_string()),
        "the child keeps the tools that need no grant (the kernel builtins and wait): {child:?}"
    );
}

/// D-74: a `[tools.<name>] kind = "mcp"` entry of the user catalog *is* the user's
/// binding of that service, so its tools are on the member's model surface — named
/// `<service>_<tool>`. It used to load only when its name appeared in a bindings
/// list the product builds (files/shell/web/skills), which no user surface could
/// extend: every configured MCP service was unreachable while the docs promised the
/// `[tools.*]` section as the binding.
#[tokio::test]
async fn a_driver_that_cannot_boot_parks_the_instance_with_the_reason() {
    // The failure mode this pins (D-104): the coordinator's discovery loop spawned each ACTIVE instance's
    // driver with `?`, so a driver that could not boot (here a *required* MCP service whose secret is not in
    // the environment) ended the coordinator task. The session stayed up and silent — no event, no log line,
    // and a headless run waited out its whole deadline — while nothing drove any instance.
    let root = root("driver-boot-failure");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let seen: Seen = Arc::new(Mutex::new(HashMap::new()));
    let mut cfg = config(
        &root,
        factory_with_log(
            HashMap::from([("i-leader".to_string(), vec![Step::Message(finish_call("never runs"))])]),
            seen.clone(),
        ),
    );
    cfg.catalog.tools.insert(
        "probe".into(),
        serde_json::from_value(json!({
            "kind": "mcp", "mcp_server": "probe", "mcp_transport": "http", "mcp_execution": "host",
            "required": true, "url": "http://127.0.0.1:1/mcp",
            "bearer_token_env_var": "PROBE_DRIVER_BOOT_TOKEN_9F2A"
        }))
        .expect("a tool binding"),
    );
    let handle = start(cfg).await.expect("the coordinator stays up");
    // the instance is parked with the runtime's own words...
    let event = wait_event(&handle, "instance_lifecycle", 10_000).await;
    assert_eq!(event["payload"]["lifecycle"], json!("PARKED"), "{event}");
    let reason = event["payload"]["reason"].as_str().unwrap_or("");
    assert!(
        reason.contains("PROBE_DRIVER_BOOT_TOKEN_9F2A") && reason.contains("probe"),
        "the park reason names the service and the missing variable: {event}"
    );
    // ...and the coordinator is still driving: the instance is not retried in a storm, and no turn was opened
    tokio::time::sleep(Duration::from_millis(300)).await;
    let events = handle.events(0).await.unwrap();
    assert_eq!(
        events.iter().filter(|e| e["kind"] == json!("instance_lifecycle")).count(),
        1,
        "a parked instance is not parked again and again: {events:?}"
    );
    assert!(
        !events.iter().any(|e| e["kind"] == json!("request_began")),
        "nothing opens a turn for an instance whose driver cannot boot: {events:?}"
    );
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn a_configured_mcp_service_reaches_the_members_surface() {
    // a minimal stdio server: initialize + tools/list, one tool
    let server = r#"
import json, sys
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request: continue
    if request['method'] == 'initialize':
        result = {'protocolVersion': '2025-06-18'}
    elif request['method'] == 'tools/list':
        result = {'tools': [{'name': 'ping', 'description': 'answers pong',
                             'inputSchema': {'type': 'object', 'properties': {}}}]}
    else:
        result = {}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#;
    let root = root("mcp-bound");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let seen: Seen = Arc::new(Mutex::new(HashMap::new()));
    let mut cfg = config(
        &root,
        factory_with_log(
            HashMap::from([("i-leader".to_string(), vec![Step::Message(finish_call("done"))])]),
            seen.clone(),
        ),
    );
    cfg.catalog.tools.insert(
        "probe".into(),
        serde_json::from_value(json!({"kind": "mcp", "mcp_execution": "host", "command": "/usr/bin/python3",
                                      "args": ["-u", "-c", server]}))
        .expect("a tool binding"),
    );
    let handle = start(cfg).await.expect("start");
    handle.input("i-leader", "do the probe").await.expect("input");
    let closed = wait_event(&handle, "goal_completed", 20_000).await;
    assert_eq!(closed["payload"]["status"], json!("SUCCEEDED"), "{closed}");
    handle.shutdown().await.expect("shutdown");

    let seen = seen.lock().unwrap().clone();
    let offered = seen.get("i-leader").and_then(|requests| requests.first()).cloned().unwrap_or_default();
    assert!(
        offered.contains(&"probe_ping".to_string()),
        "the declared service's tool is on the member's surface: {offered:?}"
    );
    // the built-in surface is unchanged
    assert!(offered.contains(&"shell".to_string()) && offered.contains(&"finish".to_string()), "{offered:?}");
}

/// An input that arrives while a turn is in flight enters at the **next boundary**
/// (§5.4 and the model's own `Input` action: user input lands at READY). It is
/// queued, the turn in flight is not disturbed, and the model sees it *after* its
/// own reply — so its reply is never read as the answer to an input it never saw,
/// and the input always gets a turn of its own.
#[tokio::test]
async fn an_input_arriving_during_a_turn_enters_at_the_next_boundary() {
    let leader = vec![Step::Slow(1500, reply("first answer")), Step::Message(reply("second answer"))];
    let seen: Seen = Arc::new(Mutex::new(HashMap::new()));
    let root = root("mid-turn-input");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let handle =
        start(config(&root, factory_with_log(HashMap::from([("i-leader".to_string(), leader)]), seen.clone())))
            .await
            .expect("start");
    handle.input("i-leader", "first question").await.expect("input");
    // the request is logged before the provider's delay, so this is inside the turn
    for _ in 0..400 {
        if seen.lock().unwrap().get("i-leader").map(Vec::len).unwrap_or(0) >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(seen.lock().unwrap().get("i-leader").map(Vec::len), Some(1), "the first turn is in flight");
    let queued = handle
        .submit_user(cmd(
            "mid-turn-input",
            "submit_input",
            json!({"instance_id": "i-leader", "envelope_id": "env-mid-turn", "text": "second question"}),
        ))
        .await
        .expect("input");
    assert_eq!(queued["applied"], json!(false), "an input inside a turn waits for the boundary: {queued}");
    assert_eq!(queued["queued"], json!(true), "{queued}");
    // the turn ends, the boundary applies the input and a new turn answers it
    for _ in 0..600 {
        if seen.lock().unwrap().get("i-leader").map(Vec::len).unwrap_or(0) >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        seen.lock().unwrap().get("i-leader").map(Vec::len),
        Some(2),
        "the queued input must open a turn of its own"
    );
    handle.shutdown().await.expect("shutdown");
    // and it entered the conversation *after* the reply it was not part of
    let control = second_control(&root);
    let rows: Vec<(i64, String, String)> = control
        .connection()
        .prepare("SELECT idx, kind, message_json FROM context_entries WHERE instance_id = 'i-leader' ORDER BY idx")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    let index_of = |needle: &str| {
        rows.iter()
            .find(|(_, _, message)| message.contains(needle))
            .map(|(idx, _, _)| *idx)
            .unwrap_or_else(|| panic!("{needle:?} is missing from the conversation: {rows:?}"))
    };
    let reply_index = index_of("first answer");
    let input_index = index_of("second question");
    assert!(
        input_index > reply_index,
        "the input entered behind the reply it was not part of (input {input_index}, reply {reply_index})"
    );
}

/// The other direction of D-60's check: a grant the *user* issues reaches the
/// worker's model-visible surface at its next request, and revoking it takes the
/// tool away again (D-61; V2Authority's `GrantReachesTheSurface` /
/// `RevokedToolLeavesTheSurface`). A spawned worker holds no `shell@workspace`
/// (§5.1) and nothing in the product could hand it one before the authority
/// surface existed. The grant goes through `SupervisorHandle::submit_user`, the
/// same single-writer path the daemon's `authority` client uses, and the surface
/// is recomputed per request (`driver::team_kernel`) — exactly the freshness the
/// model's `Observe` action assumes.
#[tokio::test]
async fn a_users_grant_reaches_the_workers_surface_at_the_next_request() {
    /// Wait until `instance` has made at least `count` requests.
    async fn wait_requests(seen: &Seen, instance: &str, count: usize, timeout_ms: u64) {
        for _ in 0..(timeout_ms / 25) {
            if seen.lock().unwrap().get(instance).map(Vec::len).unwrap_or(0) >= count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("instance {instance} did not make {count} requests within {timeout_ms}ms");
    }
    /// Wait until the instance's turn is over (its own text is the last context
    /// entry). A request appears in `Seen` before its response is imported, and an
    /// input submitted while the turn is still in flight is stored *behind* that
    /// text — it then gets no turn of its own, because the driver opens no new
    /// turn when the last entry is the model's own text (§5.4,
    /// `NoTurnWithoutWork`). So a test that feeds two inputs must wait here.
    async fn wait_turn_end(root: &Root, instance: &str, timeout_ms: u64) {
        for _ in 0..(timeout_ms / 25) {
            let kind = second_control(root).connection().query_row(
                "SELECT kind FROM context_entries WHERE instance_id = ?1 ORDER BY idx DESC LIMIT 1",
                [instance],
                |row| row.get::<_, String>(0),
            );
            if kind.as_deref() == Ok("assistant") {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("instance {instance} did not finish its turn within {timeout_ms}ms");
    }
    let leader = vec![
        Step::Message(json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": "spawn",
             "arguments": json!({"instance_id": "i-worker", "instructions": "wait for a message"}).to_string()}},
            {"id": "c2", "type": "function", "function": {"name": "send",
             "arguments": json!({"recipient": "i-worker", "text": "introduce yourself"}).to_string()}}
        ]})),
        Step::Message(reply("worker ready")),
    ];
    // The worker only ever answers plainly: this test is about what it is
    // *offered*. A spawn alone opens no turn (§5.4, `NoTurnWithoutWork`), so the
    // leader's message is what produces its first request; a delegated task would
    // keep it busy (an open task is always work) and its extra requests would make
    // the "next request" assertions ambiguous.
    let worker = vec![Step::Message(reply("idle")), Step::Message(reply("idle")), Step::Message(reply("idle"))];
    let seen: Seen = Arc::new(Mutex::new(HashMap::new()));
    let root = root("user-grant-surface");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let handle = start(config(
        &root,
        factory_with_log(
            HashMap::from([("i-leader".to_string(), leader), ("i-worker".to_string(), worker)]),
            seen.clone(),
        ),
    ))
    .await
    .expect("start");
    let requests = || seen.lock().unwrap().get("i-worker").cloned().unwrap_or_default();
    handle.input("i-leader", "spawn a worker and greet it").await.expect("input");
    wait_requests(&seen, "i-worker", 1, 20_000).await;
    assert!(!requests()[0].contains(&"shell".to_string()), "the spawned worker starts without shell: {:?}", requests());
    wait_turn_end(&root, "i-worker", 20_000).await;

    // the user grants the shared-workspace shell through the authority surface
    // (the same `submit_user` path the daemon's `authority` client uses)
    let granted = handle
        .submit_user(cmd(
            "user-grant-shell",
            "issue_grant",
            json!({"subject": "i-worker", "action": "shell", "resource_scope": "workspace"}),
        ))
        .await
        .expect("grant");
    let grant_id = granted["grant_id"].as_str().expect("a grant id").to_string();
    let after_first = requests().len();
    handle.input("i-worker", "run the project tests").await.expect("input");
    wait_requests(&seen, "i-worker", after_first + 1, 20_000).await;
    let offered = requests()[after_first].clone();
    assert!(
        offered.contains(&"shell".to_string()),
        "the grant must reach the worker's next request: {offered:?} (all: {:?})",
        requests()
    );
    wait_turn_end(&root, "i-worker", 20_000).await;

    // and taking it back removes the tool from every request prepared after it
    let revoked = handle
        .submit_user(cmd("user-revoke-shell", "revoke_grant", json!({"grant_id": grant_id})))
        .await
        .expect("revoke");
    assert_eq!(revoked["revoked"].as_array().map(Vec::len), Some(1), "{revoked}");
    let before_revoke_turn = requests().len();
    handle.input("i-worker", "carry on").await.expect("input");
    wait_requests(&seen, "i-worker", before_revoke_turn + 1, 20_000).await;
    let after_revoke = requests()[before_revoke_turn..].to_vec();
    assert!(
        after_revoke.iter().all(|offered| !offered.contains(&"shell".to_string())),
        "a revoked grant must leave the surface again: {after_revoke:?} (all: {:?})",
        requests()
    );
    handle.shutdown().await.expect("shutdown");
}

/// A settled goal leaves no product surface to open a new one (known gap, D-64's
/// audit): the kernel offers no create-goal tool, `delegate_task` requires an ACTIVE
/// goal, and the runtime creates no goal when a later input arrives — so the second
/// instruction of a session cannot build a team, and the model is told (in its own
/// protocol note) to "create a new goal first" without a way to do it.
///
/// This pins the *current* behaviour so that changing it is deliberate. Whether the
/// runtime should open a goal per user input, or the Leader should be given a way to
/// open one, is a design decision for the user (recorded in docs/ACCEPTANCE.md).
#[tokio::test]
async fn a_settled_goal_leaves_a_later_delegation_without_an_active_goal() {
    let leader = vec![
        Step::Message(finish_call("first goal done")),
        Step::Message(json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": "delegate",
             "arguments": json!({"assignee": "i-worker", "task_id": "t-late", "description": "later work"}).to_string()}}]})),
    ];
    let worker = vec![Step::Message(reply("should never run"))];
    let seen: Seen = Arc::new(Mutex::new(HashMap::new()));
    let root = root("settled-goal-delegation");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let handle = start(config(
        &root,
        factory_with_log(
            HashMap::from([("i-leader".to_string(), leader), ("i-worker".to_string(), worker)]),
            seen.clone(),
        ),
    ))
    .await
    .expect("start");
    handle.input("i-leader", "do the first thing").await.expect("input");
    let closed = wait_event(&handle, "goal_completed", 20_000).await;
    assert_eq!(closed["payload"]["status"], json!("SUCCEEDED"), "{closed}");
    // the worker the second turn delegates to must exist, or the refusal would be
    // about the assignee instead of the goal (the question under test). The write goes
    // through the supervisor (the daemon's own path) rather than a second `Control`
    // connection: two writers on one SQLite file can lose the writer race under a
    // loaded suite, and the product serializes its writers through this one.
    handle
        .submit_user(cmd(
            "late-worker",
            "create_instance",
            json!({"id": "i-worker", "workspace_ref": root.dir.join("ws")}),
        ))
        .await
        .expect("create the worker");
    // a second instruction: the runtime opens no goal for it
    handle.input("i-leader", "now delegate some work").await.expect("input");
    for _ in 0..800 {
        if seen.lock().unwrap().get("i-leader").map(Vec::len).unwrap_or(0) >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        seen.lock().unwrap().get("i-leader").map(Vec::len).unwrap_or(0) >= 2,
        "the second instruction really opens a turn: {:?}",
        seen.lock().unwrap().get("i-leader")
    );
    let control = second_control(&root);
    // The delegation the model attempted must report the missing goal instead of spawning a
    // member for work that has nowhere to be charged. The receipt lands *after* the model's
    // answer, so wait for it rather than reading once after a fixed sleep: under a loaded
    // suite (every test binary in parallel) the receipt had not arrived and the read saw only
    // the first turn's `finish` receipt — the same class of race the deadline fixture had
    // (D-83). 20 s is the same bound the waits above use.
    let read_results = |control: &teamagents_core::v2::Control| -> Vec<String> {
        control
            .connection()
            .prepare(
                "SELECT message_json FROM context_entries
                 WHERE instance_id = 'i-leader' AND kind = 'tool_result' ORDER BY idx",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut results = read_results(&control);
    while !results.iter().any(|entry| entry.contains("not active")) && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
        results = read_results(&control);
    }
    let goals: Vec<(String, String)> = control
        .connection()
        .prepare("SELECT id, status FROM goals ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert_eq!(goals.len(), 1, "no new goal is created for the second input: {goals:?}");
    assert_eq!(goals[0].1, "SUCCEEDED", "{goals:?}");
    let refusal = results
        .iter()
        .find(|entry| entry.contains("not active"))
        .cloned()
        .unwrap_or_else(|| panic!("no delegation refusal in the receipts: {results:?}"));
    assert!(
        refusal.contains("is not active") && refusal.contains("create_goal"),
        "the refusal says the goal is closed and names the missing step: {refusal}"
    );
    assert_eq!(seen.lock().unwrap().get("i-worker").map(Vec::len), None, "the worker never runs: no task reached it");
    handle.shutdown().await.expect("shutdown");
}

/// A worker whose model ends its turn with prose instead of settling its task goes
/// **idle** with the task still RUNNING — the runtime does not ask the same question
/// again (D-65; V2Control's `NoTurnWithoutWork` forbids the state the old clause
/// produced: a new request while the last entry is the model's own text).
///
/// The delegator is then the one who resolves it: cancelling the task satisfies its
/// wait (a BLOCKED task would not), so the leader wakes, reports, and the goal
/// settles honestly instead of the session burning a budget on a loop.
#[tokio::test]
async fn a_prose_reply_leaves_one_turn_and_the_delegator_resolves_the_task() {
    let leader = vec![
        Step::Message(json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": "spawn",
             "arguments": json!({"instance_id": "i-worker", "instructions": "do the task"}).to_string()}},
            {"id": "c2", "type": "function", "function": {"name": "delegate",
             "arguments": json!({"assignee": "i-worker", "task_id": "t-prose", "description": "write the file"}).to_string()}}
        ]})),
        Step::Message(wait_call("c3", json!({"mode": "ANY", "conditions": [{"kind": "task", "task_id": "t-prose"}]}))),
        Step::Message(finish_call("the worker stopped; the task was cancelled")),
    ];
    // the worker answers with prose and never calls finish: its turn ends unsettled
    let worker = vec![Step::Message(reply("I have not finished; see my notes above."))];
    let seen: Seen = Arc::new(Mutex::new(HashMap::new()));
    let root = root("prose-reply-idle");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let handle = start(config(
        &root,
        factory_with_log(
            HashMap::from([("i-leader".to_string(), leader), ("i-worker".to_string(), worker)]),
            seen.clone(),
        ),
    ))
    .await
    .expect("start");
    handle.input("i-leader", "delegate the file to a worker").await.expect("input");
    // the worker makes exactly one request, and stays idle afterwards
    for _ in 0..800 {
        if seen.lock().unwrap().get("i-worker").map(Vec::len).unwrap_or(0) >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(seen.lock().unwrap().get("i-worker").map(Vec::len), Some(1), "the worker runs one turn");
    let worker = "i-worker";
    // a long moment: the old rule re-asked the model here (169 times in the real run)
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        seen.lock().unwrap().get("i-worker").map(Vec::len),
        Some(1),
        "a prose reply does not open another turn: {:?}",
        seen.lock().unwrap().get("i-worker")
    );
    let status = |task: &str| -> Option<String> {
        second_control(&root)
            .connection()
            .query_row("SELECT status FROM tasks WHERE id = ?1", [task], |row| row.get(0))
            .ok()
    };
    assert_eq!(status("t-prose").as_deref(), Some("RUNNING"), "the task stays open for the delegator to resolve");
    let phases = |instance: &str| -> Option<String> {
        second_control(&root)
            .connection()
            .query_row("SELECT phase FROM instances WHERE id = ?1", [instance], |row| row.get(0))
            .ok()
    };
    assert_eq!(phases("i-leader").as_deref(), Some("WAITING"), "the delegator waits on its task");
    assert_eq!(phases(worker).as_deref(), Some("READY"), "and the worker is idle, not spinning");
    // the user resolves it the way the operating notes describe: cancel the task — through
    // the supervisor's user path, the same one `teamagents tasks cancel` uses
    handle
        .submit_user(cmd("cancel-late", "cancel_task", json!({"task_id": "t-prose", "reason": "the assignee stopped"})))
        .await
        .expect("cancel the task");
    assert_eq!(status("t-prose").as_deref(), Some("CANCELLED"));
    // the delegator's wait is satisfied by the cancellation and it finishes honestly
    let closed = wait_event(&handle, "goal_completed", 20_000).await;
    assert_eq!(closed["payload"]["status"], json!("SUCCEEDED"), "{closed}");
    assert_eq!(seen.lock().unwrap().get("i-leader").map(Vec::len), Some(3), "the leader ran its three turns");
    handle.shutdown().await.expect("shutdown");
}
