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
use teamagents_core::models::UserConfig;
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

fn config(
    root: &Root,
    provider_factory: impl Fn(&str, &KernelProfile) -> ScriptedProvider,
) -> SupervisorConfig<ScriptedProvider, impl Fn(&str, &KernelProfile) -> ScriptedProvider> {
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
