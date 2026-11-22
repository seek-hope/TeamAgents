//! R2-P3 supervisor end-to-end (scripted providers, no real model): one
//! coordinator discovers and drives every ACTIVE instance; spawned workers
//! join mid-run; collaboration (spawn/delegate/wait/settle) executes through
//! the control plane; termination retires drivers and lets the loop exit.

use serde_json::{json, Value as Json};
use std::collections::{HashMap, VecDeque};
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
    /// Hold a reply until the test commits the action the next request must observe.
    Held(Arc<tokio::sync::Notify>, Json),
    /// Stream text deltas (a transient preview) and then hold the request open for `u64` ms before replying,
    /// so a test can observe the preview while the turn runs (D-366).
    Deltas(Vec<String>, u64, Json),
    /// Inspect the prepared request (including tool receipts) to construct the next reply.
    Dynamic(Box<dyn FnOnce(&ModelRequest) -> Json + Send>),
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
        cancel: &Cancel,
        on_event: &mut (dyn FnMut(ProviderEvent) + Send),
    ) -> Result<AttemptOutcome, ProviderError> {
        self.seen.lock().unwrap().entry(self.instance.clone()).or_default().push(
            request.tools.iter().filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string)).collect(),
        );
        let next = self.script.lock().unwrap().pop_front().unwrap_or(Step::Message(reply("script exhausted")));
        let (message, delay) = match next {
            Step::Message(message) => (message, 0),
            Step::Slow(ms, message) => (message, ms),
            Step::Held(release, message) => {
                tokio::select! {
                    _ = release.notified() => {}
                    _ = cancel.cancelled() => return Err(ProviderError::interrupted("interrupted by the test")),
                }
                (message, 0)
            }
            Step::Deltas(deltas, ms, message) => {
                for delta in deltas {
                    on_event(ProviderEvent::TextDelta(delta));
                }
                (message, ms)
            }
            Step::Dynamic(build) => (build(request), 0),
        };
        if delay > 0 {
            // A held request honours the turn's cancel token (D-363): the interrupt test needs the turn to end
            // now, not at the provider's own pace.
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(delay)) => {}
                _ = cancel.cancelled() => return Err(ProviderError::interrupted("interrupted by the test")),
            }
        }
        match Step::Message(message) {
            Step::Message(message) => Ok(AttemptOutcome {
                response: ModelResponse {
                    message,
                    usage: Some(Usage { prompt: 3, completion: 2, total: 5, ..Default::default() }),
                    native: json!({}),
                },
                raw: json!({"scripted": true}),
                elapsed_ms: 1,
                timing: teamagents_engine::providers::StreamTimingReport::default(),
            }),
            Step::Slow(..) | Step::Held(..) | Step::Deltas(..) | Step::Dynamic(..) => {
                unreachable!("the step was already resolved")
            }
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

mod common;
#[path = "cases/supervisor_robustness.rs"]
mod supervisor_robustness;

/// The scratch root of one test: `teamagents-v2-supervisor-<tag>-<uuid>`, removed when the test ends (D-226).
type Root = common::TempRoot;

fn root(tag: &str) -> Root {
    Root::new(&format!("v2-supervisor-{tag}"))
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
            images: false,
            tool_output_chars: None,
            context_keep_chars: None,
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
        approvals_reviewer: "user".into(),
        reviewer_model: None,
        catalog: UserConfig::default(),
        catalog_base: None,
        bindings: vec![],
        max_retries: 2,
        storage_queue: 64,
        poll: Duration::from_millis(15),
        work_limits: json!({}),
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

/// The settlement of one task: the durable fact a per-instance *request count*
/// cannot give, because the settling worker's own turn continues with one more
/// request (`response_imported … READY`, measured) and the count is 1 or 2 by timing.
async fn wait_task_settled(handle: &SupervisorHandle, task_id: &str, timeout_ms: u64) -> Json {
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
    panic!("task {task_id} did not settle within {timeout_ms}ms");
}

#[tokio::test]
async fn spawned_worker_settles_and_the_leader_completes() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    for temporary in [false, true] {
        settled_worker(temporary).await;
    }
}

async fn settled_worker(temporary: bool) {
    let root = root("spawn-loop");
    let mut spawn_args = json!({"instance_id":"i-worker", "instructions":"math helper", "workspace":"isolated"});
    if temporary {
        spawn_args["lifetime"] = json!("temporary");
    }
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    // leader: spawn the worker, delegate with an explicit task id, wait on
    // the task, finish. worker: one finish settles the delegated task.
    let leader = vec![
        Step::Message(json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": "spawn",
             "arguments": spawn_args.to_string()}},
            {"id": "c2", "type": "function", "function": {"name": "delegate",
             "arguments": json!({"assignee": "i-worker", "task_id": "t-answer", "description": "what is 2+2"}).to_string()}}
        ]})),
        Step::Message(wait_call("c3", json!({"mode": "ANY", "conditions": [{"kind": "task", "task_id": "t-answer"}]}))),
        Step::Message(finish_call("team answered 4")),
    ];
    let worker = vec![Step::Message(finish_call("4"))];
    // A driver owns one provider. A Weak request log lets this test observe the actual drop,
    // rather than inferring driver exit from the persisted lifecycle alone.
    let worker_log: Seen = Default::default();
    let driver = Arc::downgrade(&worker_log);
    let first_log = Mutex::new(Some(worker_log));
    let scripts = factory(HashMap::from([("i-leader".to_string(), leader), ("i-worker".to_string(), worker)]));
    let mut cfg = config(&root, move |id: &str, profile: &KernelProfile| {
        let mut p = scripts(id, profile);
        if id == "i-worker" {
            p.seen = first_log.lock().unwrap().take().expect("no automatic respawn");
        }
        p
    });
    cfg.catalog.default_task_capabilities = vec!["shell@workspace".into()];
    let handle = start(cfg).await.expect("start");
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
    control
        .submit(
            cmd(
                "g-task-shell",
                "issue_grant",
                json!({"subject":"i-leader","action":"shell","resource_scope":"workspace","delegable":true}),
            ),
            teamagents_core::v2::Identity::User,
        )
        .unwrap();
    handle.input("i-leader", "get me 2+2 via a worker").await.expect("input");
    let closed = wait_event(&handle, "work_completed", 20_000).await;
    assert_eq!(closed["payload"]["status"], json!("SUCCEEDED"));
    // the spawned worker joined, ran and settled the delegated task
    let task: String =
        control.connection().query_row("SELECT status FROM tasks WHERE id = 't-answer'", [], |row| row.get(0)).unwrap();
    assert_eq!(task, "SUCCEEDED");
    let worker: (String, String) = control
        .connection()
        .query_row("SELECT worker_lifetime,lifecycle FROM instances WHERE id='i-worker'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    let expected = if temporary { ("temporary", "PARKED") } else { ("persistent", "ACTIVE") };
    assert_eq!(worker, (expected.0.into(), expected.1.into()));
    if temporary {
        tokio::time::timeout(Duration::from_secs(5), async {
            while driver.upgrade().is_some() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("temporary driver exited");
    } else {
        assert!(driver.upgrade().is_some(), "persistent driver still present");
    }
    let inputs = root.dir.join("state/instances/i-worker/work/INPUTS.md");
    assert!(inputs.is_file(), "workspace survives settlement");
    let count:i64 = control.connection().query_row("SELECT COUNT(*) FROM envelopes WHERE recipient='i-leader' AND kind='task_result' AND correlation_id='t-answer' AND state='APPLIED'",[],|r|r.get(0)).unwrap();
    assert_eq!(count, 1);
    let children: i64 = control.connection().query_row("SELECT count(*) FROM grants WHERE subject='i-worker' AND action='shell' AND task_id='t-answer' AND parent_grant_id IS NOT NULL AND delegable=0 AND revoked_at IS NOT NULL",[],|r|r.get(0)).unwrap();
    assert_eq!(children, 1);
    let snapshot = handle.snapshot().await.unwrap();
    let ids: Vec<&str> = snapshot["instances"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"i-leader") && ids.contains(&"i-worker"), "{ids:?}");
    handle.shutdown().await.expect("shutdown");
    let boots = Arc::new(Mutex::new(Vec::new()));
    let log = boots.clone();
    let provider = factory(HashMap::new());
    let handle = start(config(&root, move |id: &str, p: &KernelProfile| {
        log.lock().unwrap().push(id.to_string());
        provider(id, p)
    }))
    .await
    .unwrap();
    // Both rows are scanned in one discovery pass. Wait for a live Leader round trip.
    handle.input("i-leader", "Report the result already returned.").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5),async {
        loop {
            let restarted: bool=control.connection().query_row("SELECT EXISTS(SELECT 1 FROM context_entries WHERE instance_id='i-leader' AND kind='assistant' AND message_json LIKE '%script exhausted%')",[],|r|r.get(0)).unwrap();
            if restarted {break;}
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }).await.expect("new Leader responded");
    handle.shutdown().await.unwrap();
    let starts = boots.lock().unwrap();
    assert_eq!(starts.iter().filter(|id| *id == "i-worker").count(), usize::from(!temporary));
    assert!(inputs.is_file());
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
    let closed = wait_event(&handle, "work_completed", 20_000).await;
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
async fn explicit_profile_switch_routes_a_shared_wire_name_to_the_selected_endpoint() {
    let root = root("profile-destination");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let first=FakeServer::start(vec![sse_response("data: {\"choices\":[{\"delta\":{\"content\":\"first reply\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")]).await;
    let second=FakeServer::start(vec![sse_response("data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"second reply\"}]},\"finishReason\":\"STOP\"}]}\n\n")]).await;
    let mut catalog = UserConfig::default();
    for (key, proto, base, temperature, images) in
        [("one", "chat/completions", &first.base, 0.1, true), ("two", "gemini", &second.base, 0.9, false)]
    {
        catalog.models.insert(key.into(), serde_json::from_value(json!({"provider":key,"protocol":proto,"model":"shared-wire","base_url":base,"generation_options":{"temperature":temperature},"images":images,"max_retries":0})).unwrap());
    }
    let factory_catalog = catalog.clone();
    let mut cfg = config(&root, move |_, profile| {
        teamagents_engine::providers::build_for_model(&factory_catalog, &profile.model).unwrap()
    });
    cfg.leader_profile.model = "one".into();
    cfg.catalog = catalog;
    let h = start(cfg).await.unwrap();
    let c = second_control(&root);
    h.input("i-leader", "first question").await.unwrap();
    wait_model_request_complete(&c, "i-leader").await;
    wait_instance_phase(&h, "i-leader", "READY", 5000).await;
    assert!(second.bodies().is_empty());
    h.submit_user(cmd("switch", "set_instance_profile", json!({"instance_id":"i-leader","model":"two"})))
        .await
        .unwrap();
    h.input("i-leader", "second question").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), second.task).await.unwrap().unwrap();
    for _ in 0..200 {
        if c.connection()
            .query_row("SELECT count(*) FROM model_requests WHERE status='COMPLETE'", [], |r| r.get::<_, i64>(0))
            .unwrap()
            == 2
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    // Routing is proved by the wire content below, not by a tariff receipt: billing was removed, so the
    // `pricing_json` column this used to read no longer exists. Two completed requests must still exist.
    let completed: i64 = c
        .connection()
        .query_row("SELECT count(*) FROM model_requests WHERE status='COMPLETE'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(completed, 2, "both turns reached a COMPLETE request");
    let one: Json = serde_json::from_str(&first.bodies()[0]).unwrap();
    let two: Json = serde_json::from_str(&second.bodies.lock().unwrap()[0]).unwrap();
    assert_eq!(one["model"], "shared-wire");
    assert_eq!(one["temperature"], 0.1);
    assert!(one.to_string().contains("view_image"));
    assert!(!two["tools"].to_string().contains("view_image"));
    assert!(two["contents"].to_string().contains("second question"));
    assert_eq!(two["generationConfig"]["temperature"], 0.9);
    assert_eq!(first.bodies().len(), 1);
    first.task.await.unwrap();
    h.shutdown().await.unwrap();
}

/// Same wire ID on two loopback accounts. The B fixture has a deliberately small native
/// window to exercise compaction; its first normal turn also needs a transient retry.
#[tokio::test]
async fn shared_worker_routes_survive_pin_follow_leader_change_and_restart() {
    use teamagents_engine::providers::AnyProvider;
    let root = root("worker-routes");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let frame = sse_response(
        "data: {\"choices\":[{\"delta\":{\"content\":\"receipt\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
    );
    let a = FakeServer::start(vec![frame.clone(), frame.clone()]).await;
    let b = FakeServer::start(vec![
        "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".into(),
        frame.clone(),
        frame.clone(),
        frame,
    ])
    .await;
    let mut catalog = UserConfig::default();
    for (key, base, window) in [("one", &a.base, 128000), ("two", &b.base, 512)] {
        catalog.models.insert(key.into(),serde_json::from_value(json!({"provider":"fixture","protocol":"chat/completions","model":"same-wire","base_url":base,"context_window":window,"max_retries":1})).unwrap());
    }
    let cfg = || {
        let mut cfg = config(&root, |_, _| AnyProvider::Unavailable { reason: "not bound".into() });
        cfg.leader_profile.model = "one".into();
        cfg.catalog = catalog.clone();
        cfg
    };
    let h = start(cfg()).await.unwrap();
    h.submit_user(cmd("worker", "spawn_instance", json!({"instance_id":"w"}))).await.unwrap();
    let c = second_control(&root);
    async fn turn(h: &SupervisorHandle, c: &teamagents_core::v2::Control, id: &str, prompt: &str, n: i64) {
        h.input(id, prompt).await.unwrap();
        tokio::time::timeout(Duration::from_secs(12),async {
            loop {
                let count:i64=c.connection().query_row("SELECT COUNT(*) FROM model_requests WHERE instance_id=?1 AND kind='turn' AND status='COMPLETE'", [id], |r|r.get(0)).unwrap();
                if count == n { break; }
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        }).await.expect("normal turn did not complete");
        wait_instance_phase(h, id, "READY", 5000).await;
    }
    turn(&h, &c, "w", "initial follow", 1).await;
    h.submit_user(cmd("pin-same", "set_role_model", json!({"role":"workers","model":"one"}))).await.unwrap();
    h.submit_user(cmd("leader-two", "set_role_model", json!({"role":"leader","model":"two"}))).await.unwrap();
    turn(&h, &c, "w", "still pinned", 2).await;
    assert!(b.bodies().is_empty());
    h.submit_user(cmd("follow", "set_role_model", json!({"role":"workers","follow":true}))).await.unwrap();
    h.shutdown().await.unwrap();
    let h = start(cfg()).await.unwrap();
    h.submit_user(cmd("another-worker", "spawn_instance", json!({"instance_id":"w2"}))).await.unwrap();
    turn(&h, &c, "w2", "new worker", 1).await;
    turn(&h, &c, "w", &"compact old turns ".repeat(400), 3).await;
    let compressed: i64 = c
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM model_requests WHERE instance_id='w' AND kind='compression' AND status='COMPLETE'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(compressed, 1);
    let bodies = b.bodies();
    assert_eq!(bodies.len(), 4);
    assert_eq!(bodies[0], bodies[1], "retry changed prepared body");
    assert!(bodies[0].contains("new worker"));
    assert!(bodies[2].contains("initial follow") && bodies[2].contains("still pinned"));
    assert!(bodies[3].contains("compact old turns"));
    assert_eq!(a.bodies().len(), 2);
    assert!(a
        .bodies()
        .iter()
        .chain(&bodies)
        .all(|body| serde_json::from_str::<Json>(body).unwrap()["model"] == "same-wire"));
    h.shutdown().await.unwrap();
    a.task.await.unwrap();
    b.task.await.unwrap();
}

#[tokio::test]
async fn unique_legacy_leader_binds_once_and_ambiguous_names_never_bind() {
    for ambiguous in [true, false] {
        let root = root("legacy-selection");
        std::fs::create_dir_all(root.dir.join("ws")).unwrap();
        let mut catalog = UserConfig::default();
        let entry = serde_json::from_value::<ModelProfile>(json!({"provider":"fixture","model":"same-wire"})).unwrap();
        catalog.models.insert("one".into(), entry.clone());
        if ambiguous {
            catalog.models.insert("two".into(), entry.clone());
        }
        let build = |catalog: UserConfig, key: &str| {
            let mut cfg = config(&root, factory(HashMap::new()));
            cfg.leader_profile.model = key.into();
            cfg.catalog = catalog;
            cfg
        };
        start(build(catalog.clone(), "one")).await.unwrap().shutdown().await.unwrap();
        let c = second_control(&root);
        c.connection()
            .execute_batch(
                "UPDATE instances SET profile_json=json_remove(profile_json,'$.catalog_key') WHERE is_leader=1",
            )
            .unwrap();
        start(build(catalog.clone(), "one")).await.unwrap().shutdown().await.unwrap();
        let roles = || teamagents_core::v2::control::role_model_snapshot(c.connection()).unwrap();
        if ambiguous {
            assert!(roles()["leader"].is_null());
        } else {
            assert_eq!(roles()["leader"], "one");
            catalog.models.remove("one");
            catalog.models.insert("two".into(), entry);
            start(build(catalog, "two")).await.unwrap().shutdown().await.unwrap();
            assert_eq!(roles()["leader"], "one");
        }
    }
}

#[tokio::test]
async fn private_continuation_stays_with_its_endpoint_on_a_same_protocol_switch() {
    let root = root("private-route");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let frame = |answer: &str| {
        sse_response(&format!(
            "data: {}\n\ndata: [DONE]\n\n",
            json!({"choices":[{"delta":{"content":answer,"reasoning_content":"private-bound-fixture"},"finish_reason":"stop"}]})
        ))
    };
    let a = FakeServer::start(vec![frame("public-first"), frame("public-second")]).await;
    let b = FakeServer::start(vec![frame("public-third")]).await;
    let mut catalog = UserConfig::default();
    for (key, base) in [("a", &a.base), ("b", &b.base)] {
        catalog.models.insert(
            key.into(),
            serde_json::from_value(
                json!({"provider":"fixture","protocol":"chat/completions","model":"shared","base_url":base}),
            )
            .unwrap(),
        );
    }
    let factory = catalog.clone();
    let mut cfg = config(&root, move |_, p| teamagents_engine::providers::build_for_model(&factory, &p.model).unwrap());
    cfg.catalog = catalog;
    cfg.leader_profile.model = "a".into();
    let h = start(cfg).await.unwrap();
    let control = second_control(&root);
    for (i, q) in ["one", "two", "three"].into_iter().enumerate() {
        if i == 2 {
            h.submit_user(cmd("switch", "set_instance_profile", json!({"instance_id":"i-leader","model":"b"})))
                .await
                .unwrap();
        }
        h.input("i-leader", q).await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let count: i64 = control
                .connection()
                .query_row("SELECT count(*) FROM model_requests WHERE status='COMPLETE'", [], |r| r.get(0))
                .unwrap();
            if count == 1 + i as i64 {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        wait_instance_phase(&h, "i-leader", "READY", 5000).await;
    }
    assert!(a.bodies()[1].contains("private-bound-fixture"));
    let next = &b.bodies()[0];
    assert!(next.contains("public-second"));
    assert!(!next.contains("private-bound-fixture"));
    assert!(!next.contains("teamagents_route"));
    a.task.await.unwrap();
    b.task.await.unwrap();
    h.shutdown().await.unwrap();
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
            images: false,
            provider: "deepseek".into(),
            protocol: "deepseek".into(),
            model: "ds-flash".into(),
            base_url: Some(chat_server.base.clone()),
            api_key_env: None,
            timeout: 5,
            max_retries: 0,
            generation_options: Default::default(),
            efforts: Vec::new(),
            context_window: Some(1_000_000),
            codex_profile: None,
        },
    );
    catalog.models.insert(
        "worker-model".into(),
        ModelProfile {
            images: false,
            provider: "openai".into(),
            protocol: "responses".into(),
            model: "gpt-x".into(),
            base_url: Some(responses_server.base.clone()),
            api_key_env: None,
            timeout: 5,
            max_retries: 0,
            generation_options: Default::default(),
            efforts: Vec::new(),
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
    handle
        .submit_user(cmd("pin", "set_role_model", json!({"role":"workers","model":"worker-model"})))
        .await
        .expect("preselect Workers");
    let mut control = second_control(&root);
    control
        .submit(
            cmd(
                "mk-worker",
                "create_instance",
                json!({"id": "i-worker", "workspace_ref": root.dir.join("ws").to_string_lossy()}),
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

/// §5.1 wiring: a terminated instance's isolated workspace is retired by the
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
    let closed = wait_event(&handle, "work_completed", 20_000).await;
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
    let closed = wait_event(&handle, "work_completed", 20_000).await;
    assert_eq!(closed["payload"]["status"], json!("SUCCEEDED"), "{closed}");
    handle.shutdown().await.expect("shutdown");

    let seen = seen.lock().unwrap().clone();
    let offered = seen.get("i-leader").and_then(|requests| requests.first()).cloned().unwrap_or_default();
    assert!(
        offered.contains(&"codemode".to_string()),
        "the declared MCP service is reachable through the codemode tool on the member's surface: {offered:?}"
    );
    assert!(
        !offered.contains(&"probe_ping".to_string()),
        "the MCP tool itself is not advertised individually (D-374): {offered:?}"
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

/// D-363: the user's interrupt cancels the running turn, and the instruction queued behind it then gets a turn
/// of its own. The cancelled turn's answer never reaches the context — the provider is cancelled mid-flight
/// and the late reply is not imported (`V2Control`'s `CancelRequest`, A13).
#[tokio::test]
async fn interrupting_a_turn_cancels_it_and_the_queued_input_takes_over() {
    let leader = vec![
        Step::Slow(30_000, reply("held answer that must not be applied")),
        Step::Message(reply("redirected answer")),
    ];
    let seen: Seen = Arc::new(Mutex::new(HashMap::new()));
    let root = root("interrupt-turn");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let handle =
        start(config(&root, factory_with_log(HashMap::from([("i-leader".to_string(), leader)]), seen.clone())))
            .await
            .expect("start");
    handle.input("i-leader", "first question").await.expect("input");
    for _ in 0..400 {
        if seen.lock().unwrap().get("i-leader").map(Vec::len).unwrap_or(0) >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(seen.lock().unwrap().get("i-leader").map(Vec::len), Some(1), "the turn is in flight");
    // the new instruction is queued behind the running turn
    let queued = handle
        .submit_user(cmd(
            "redirect-input",
            "submit_input",
            json!({"instance_id": "i-leader", "envelope_id": "env-redirect", "text": "redirected question"}),
        ))
        .await
        .expect("input");
    assert_eq!(queued["queued"], json!(true), "{queued}");
    // the user interrupts the running turn
    let interrupted = handle
        .submit_user(cmd("interrupt-1", "interrupt_instance", json!({"instance_id": "i-leader"})))
        .await
        .expect("interrupt");
    assert_eq!(interrupted["interrupted"], json!(true), "{interrupted}");
    // the queued input opens the next turn
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
    // the cancelled answer never entered the conversation
    let control = second_control(&root);
    let messages: Vec<String> = control
        .connection()
        .prepare("SELECT message_json FROM context_entries WHERE instance_id = 'i-leader' ORDER BY idx")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert!(
        messages.iter().any(|message| message.contains("redirected answer")),
        "the redirect's answer is missing: {messages:?}"
    );
    assert!(
        !messages.iter().any(|message| message.contains("held answer")),
        "the cancelled turn's answer was applied: {messages:?}"
    );
}

/// D-366: the text an attempt streams is exposed as a transient, non-authoritative preview while the turn runs
/// and is gone once the complete response lands (§9). Nothing about it is persisted, and a client that never
/// reads it loses nothing.
#[tokio::test]
async fn a_running_turn_exposes_a_transient_preview_and_clears_it() {
    let leader = vec![Step::Deltas(vec!["Hel".into(), "lo".into()], 600, reply("Hello"))];
    let root = root("preview");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let handle = start(config(&root, factory(HashMap::from([("i-leader".to_string(), leader)])))).await.expect("start");
    handle.input("i-leader", "hi").await.expect("input");
    // while the attempt is held open the preview carries exactly the deltas streamed so far
    let mut observed = None;
    for _ in 0..200 {
        let previews = handle.previews();
        if let Some(text) =
            previews["previews"].as_array().and_then(|rows| rows.first()).and_then(|row| row["text"].as_str())
        {
            if text == "Hello" {
                observed = Some(text.to_string());
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(observed.as_deref(), Some("Hello"), "the streamed text is exposed while the turn runs");
    // once the complete response lands the preview is cleared — it never becomes a fact
    for _ in 0..400 {
        if handle.previews()["previews"].as_array().map(Vec::is_empty).unwrap_or(false) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        handle.previews()["previews"].as_array().map(Vec::is_empty).unwrap_or(false),
        "the preview clears when the turn ends"
    );
    handle.shutdown().await.expect("shutdown");
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

/// A settled work leaves no product surface to open a new one (known gap, D-64's
/// audit): the kernel offers no create-work tool, `delegate_task` requires an ACTIVE
/// work, and the runtime creates no work when a later input arrives — so the second
/// instruction of a session cannot build a team, and the model is told (in its own
/// protocol note) to "create a new work first" without a way to do it.
///
/// This pins the *current* behaviour so that changing it is deliberate. Whether the
/// runtime should open a work per user input, or the Leader should be given a way to
/// open one, is a design decision for the user (recorded in docs/ACCEPTANCE.md).
/// D-266: the other side of the test below. A settled work leaves later delegation with nowhere to charge —
/// and until now **no product surface could open a work**: the command is in the protocol and the daemon forwards
/// it as an ordinary user command (`work_scopes open`, the CLI added with this entry), but nothing in the product sent
/// it. Here the *user* opens a work, attaches it to the leader, and gives it a required check: the later
/// delegation then charges to it and the worker runs.
#[tokio::test]
async fn a_user_opened_work_lets_a_later_delegation_charge_somewhere() {
    // the check the user predefines is a real shell command, and a check runs through the runner: an
    // integration test must point the runner at the built binary (the same rule `jobs_runner.rs` follows)
    let previous = std::env::var_os("TEAMAGENTS_RUNNER_BIN");
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let leader = vec![
        Step::Message(finish_call("first work done")),
        Step::Message(json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": "delegate",
             "arguments": json!({"assignee": "i-worker", "task_id": "t-late", "description": "later work"}).to_string()}}]})),
        Step::Message(finish_call("the later work is delegated")),
    ];
    let worker = vec![Step::Message(finish_call("did the later work"))];
    let root = root("user-opened-work");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let handle = start(config(
        &root,
        factory(HashMap::from([("i-leader".to_string(), leader), ("i-worker".to_string(), worker)])),
    ))
    .await
    .expect("start");
    handle.input("i-leader", "do the first thing").await.expect("input");
    let closed = wait_event(&handle, "work_completed", 20_000).await;
    handle
        .submit_user(cmd(
            "late-worker",
            "create_instance",
            json!({"id": "i-worker", "workspace_ref": root.dir.join("ws")}),
        ))
        .await
        .expect("create the worker");
    // the user's own lever: open a work, attach it to the leader, and predefine a required check on it
    let opened = handle
        .submit_user(cmd(
            "work_scopes-open",
            "create_work",
            json!({"id": "work-later", "instance_id": "i-leader",
                   "limits": {"required_checks": [{"id": "later-tests", "command": "true"}]}}),
        ))
        .await
        .expect("the user opens a work");
    // the second instruction can now charge its work somewhere
    handle.input("i-leader", "now delegate some work").await.expect("input");
    // The fact this test is about is the worker *settling* the delegated task. A
    // per-instance request count cannot carry it: the settling worker's own turn
    // continues with one more request (`response_imported … READY`, measured), so the
    // count is 1 or 2 depending on whether the test reads before or after that turn —
    // which is how this test flaked (D-278). Wait for the runtime's settlement event,
    // then read the settled rows.
    let settled = wait_task_settled(&handle, "t-late", 20_000).await;
    // read everything first, shut the session down, and only then assert: a failed assertion inside a running
    // session leaves the supervisor's task alive and the test *hangs* instead of failing (measured here)
    let control = second_control(&root);
    let work_row: Option<(Option<String>, String, String)> = control
        .connection()
        .query_row(
            "SELECT i.active_work_id, g.status, g.limits_json FROM instances i JOIN work_scopes g ON g.id = i.active_work_id
             WHERE i.id = 'i-leader'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .ok();
    let later_task: Option<(String, String, String)> = control
        .connection()
        .query_row("SELECT work_id, status, assignee FROM tasks WHERE id = 't-late'", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .ok();
    drop(control);
    handle.shutdown().await.expect("shutdown");
    match previous {
        Some(value) => std::env::set_var("TEAMAGENTS_RUNNER_BIN", value),
        None => std::env::remove_var("TEAMAGENTS_RUNNER_BIN"),
    }
    assert_eq!(closed["payload"]["status"], json!("SUCCEEDED"), "{closed}");
    assert_eq!(opened["work_id"], json!("work-later"), "{opened}");
    assert_eq!(settled["payload"]["status"], json!("SUCCEEDED"), "{settled}");
    assert_eq!(
        settled["payload"]["assignee"],
        json!("i-worker"),
        "the worker runs the second instruction's task: {settled}"
    );
    let (attached, status, checks) = work_row.expect("the leader's active work");
    assert_eq!(attached.as_deref(), Some("work-later"));
    assert_eq!(status, "ACTIVE");
    assert!(checks.contains("later-tests"), "the user's required check rides on the work: {checks}");
    let (later_work, later_status, later_assignee) = later_task.expect("the later task row");
    assert_eq!(later_work, "work-later", "the later task is charged to the user's work");
    assert_eq!(later_status, "SUCCEEDED", "the later task settled");
    assert_eq!(later_assignee, "i-worker", "and the worker ran it");
}

#[tokio::test]
async fn a_settled_work_leaves_a_later_delegation_without_an_active_work() {
    let leader = vec![
        Step::Message(finish_call("first work done")),
        Step::Message(json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": "delegate",
             "arguments": json!({"assignee": "i-worker", "task_id": "t-late", "description": "later work"}).to_string()}}]})),
    ];
    let worker = vec![Step::Message(reply("should never run"))];
    let seen: Seen = Arc::new(Mutex::new(HashMap::new()));
    let root = root("settled-work-delegation");
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
    let closed = wait_event(&handle, "work_completed", 20_000).await;
    assert_eq!(closed["payload"]["status"], json!("SUCCEEDED"), "{closed}");
    // the worker the second turn delegates to must exist, or the refusal would be
    // about the assignee instead of the work (the question under test). The write goes
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
    // a second instruction: the runtime opens no work for it
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
    // The delegation the model attempted must report the missing work instead of spawning a
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
    let work_scopes: Vec<(String, String)> = control
        .connection()
        .prepare("SELECT id, status FROM work_scopes ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert_eq!(work_scopes.len(), 1, "no new work is created for the second input: {work_scopes:?}");
    assert_eq!(work_scopes[0].1, "SUCCEEDED", "{work_scopes:?}");
    // D-266: the refusal now names the real cause. Before the fix the driver charged a *hard-coded* work id, so
    // the model was told "work work-s-test is not active; create a new work" — pointing at a work that was
    // already closed and at a step no model-facing tool can take. With the arms resolving the requester's
    // *active* work, the same situation reads as what it is: the requester has none.
    let refusal = results
        .iter()
        .find(|entry| entry.contains("no active work"))
        .cloned()
        .unwrap_or_else(|| panic!("no delegation refusal in the receipts: {results:?}"));
    assert!(
        refusal.contains("delegate_task.work_id required: requester has no active work"),
        "the refusal names the missing work: {refusal}"
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
/// wait (a BLOCKED task would not), so the leader wakes, reports, and the work
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
    let closed = wait_event(&handle, "work_completed", 20_000).await;
    assert_eq!(closed["payload"]["status"], json!("SUCCEEDED"), "{closed}");
    assert_eq!(seen.lock().unwrap().get("i-leader").map(Vec::len), Some(3), "the leader ran its three turns");
    handle.shutdown().await.expect("shutdown");
}

/// D-265: the **delegator's** own exit from a task it delegated. The design says "the requester or the user
/// closes it" (§5.3), and until now only the user had a path to it (`teamagents tasks cancel`, the test above):
/// a leader whose worker had abandoned its part could only wait out a timer, redo the work, and settle with the
/// task still open (D-65's ceiling). This drives the new `cancel_task` tool through a scripted leader: delegate,
/// cancel, then wait on that task id — which the cancellation satisfies at registration.
#[tokio::test]
async fn the_delegator_cancels_its_own_abandoned_task_and_its_wait_resolves() {
    let leader = vec![
        Step::Message(json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": "spawn",
             "arguments": json!({"instance_id": "i-worker", "instructions": "do the task"}).to_string()}},
            {"id": "c2", "type": "function", "function": {"name": "delegate",
             "arguments": json!({"assignee": "i-worker", "task_id": "t-abandoned", "description": "write the file"}).to_string()}}
        ]})),
        Step::Message(json!({"role": "assistant", "content": "the worker stopped; closing my own task",
            "tool_calls": [{"id": "c3", "type": "function", "function": {"name": "cancel_task",
             "arguments": json!({"task_id": "t-abandoned", "reason": "the assignee ended its turn unsettled"}).to_string()}}]})),
        Step::Message(wait_call(
            "c4",
            json!({"mode": "ANY", "conditions": [{"kind": "task", "task_id": "t-abandoned"}]}),
        )),
        Step::Message(finish_call("did the part myself after closing the abandoned task")),
    ];
    // the worker answers with prose and never settles: the delegator's task stays RUNNING for it to close
    let worker = vec![Step::Message(reply("see my notes; I did not settle"))];
    let seen: Seen = Arc::new(Mutex::new(HashMap::new()));
    let root = root("delegator-cancel");
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
    let closed = wait_event(&handle, "work_completed", 20_000).await;
    assert_eq!(closed["payload"]["status"], json!("SUCCEEDED"), "{closed}");
    let control = second_control(&root);
    let status: String = control
        .connection()
        .query_row("SELECT status FROM tasks WHERE id = 't-abandoned'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(status, "CANCELLED", "the delegator closed its own task");
    // the assignee is told: the cancellation is an envelope, not a silent delete
    let told: i64 = control
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM envelopes WHERE recipient = 'i-worker' AND kind = 'task_cancelled'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(told, 1, "the assignee learns the task was cancelled");
    // and the tool was offered to the leader in the first place (the surface carries what the grant allows)
    let offered =
        seen.lock().unwrap().get("i-leader").and_then(|requests| requests.first()).cloned().unwrap_or_default();
    assert!(
        offered.contains(&"cancel_task".to_string()),
        "the leader is offered the cancel tool (it holds `delegate`): {offered:?}"
    );
    handle.shutdown().await.expect("shutdown");
}

/// D-143's open question, in the shape that failed live (2026-09-27): a worker whose turn is driven by a
/// **delegated task** must also see a user grant at its next request. The existing grant test drives the worker
/// with a *user input* instead ("a delegated task would keep it busy … and its extra requests would make the
/// 'next request' assertions ambiguous"), so the task-driven shape was never covered.
///
/// The live `tools/dogfood/authority.py` run of 2026-09-27 failed in exactly that shape (kept session:
/// `/tmp/teamagents-probe-harness-3/authority`): after `teamagents authority grant --subject
/// worker_shell_probe --action shell --scope workspace` (unrevoked, the worker's `workspace_ref` the shared
/// workspace, its stored profile still carrying the `shell` schema), the worker's two following task-driven
/// turns *reported* thirteen tools without `shell` and answered `blocked` twice. **This test passes for the same
/// shape** — a task-driven turn prepared after a user grant offers `shell` — so the divergence is not in the
/// supervisor path the test drives, and D-143's open question is now narrower: either the daemon-run session
/// built those requests on another path, or the worker misreported its own tool list (`est_prompt_tokens` for
/// its second turn, 2128 against 1411 for the first, is closer to "the schema was there" than to "it was not",
/// and the estimate is coarse). What would settle it is the per-request offer made observable (the open new
/// surface recorded with D-143) or one instrumented run of that session.
#[tokio::test]
async fn a_task_driven_worker_turn_sees_a_live_user_grant() {
    let leader = vec![
        Step::Message(json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": "spawn",
             "arguments": json!({"instance_id": "i-worker", "instructions": "answer plainly",
                                 "task": "say hello"}).to_string()}}]})),
        Step::Message(reply("the worker has it")),
        // after the first task settles the delegator's turn resumes: hand the worker a *second* task, which is
        // the turn that must see the grant (the live failure was exactly this turn)
        Step::Message(json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "c2", "type": "function", "function": {"name": "delegate",
             "arguments": json!({"assignee": "i-worker",
                                 "description": "run: printf granted > proof.txt"}).to_string()}}]})),
        Step::Message(reply("done")),
    ];
    // the worker settles each task with a `finish`, so the delegator wakes for the next one
    let finished = || {
        json!({"role": "assistant", "content": "", "tool_calls": [
        {"id": "f", "type": "function", "function": {"name": "finish",
         "arguments": json!({"status": "success", "summary": "hello"}).to_string()}}]})
    };
    let grant_ready = Arc::new(tokio::sync::Notify::new());
    let worker =
        vec![Step::Held(grant_ready.clone(), finished()), Step::Message(finished()), Step::Message(finished())];
    let seen: Seen = Arc::new(Mutex::new(HashMap::new()));
    let root = root("task-driven-grant");
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
    handle.input("i-leader", "spawn a worker and give it the task").await.expect("input");
    for _ in 0..800 {
        if !requests().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(!requests().is_empty(), "the task-driven worker took its first turn");
    assert!(!requests()[0].contains(&"shell".to_string()), "before the grant: {:?}", requests());

    // the user grants the shared-workspace shell (the same `submit_user` path the daemon's client uses)
    handle
        .submit_user(cmd(
            "user-grant-shell-task-driven",
            "issue_grant",
            json!({"subject": "i-worker", "action": "shell", "resource_scope": "workspace"}),
        ))
        .await
        .expect("grant");
    let before = requests().len();
    assert_eq!(before, 1, "the first task cannot finish until the grant is committed");
    grant_ready.notify_one();
    // D-407: this waits for a *scheduling* outcome (the worker's next request), not for a fact, so the bound is
    // generous and the wait polls: 20 s was enough on an idle machine and not under `make check`, where it failed
    // twice while passing alone three times out of three. The property is that the worker keeps working, not how
    // fast it does.
    for _ in 0..2_400 {
        if requests().len() > before {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let after = requests();
    assert!(after.len() > before, "the worker kept working after the grant: {after:?}");
    // D-446: waiting only for a newly recorded request raced with preparation before the grant.
    // Holding the first task's reply makes every subsequent task request prepare after the grant commits.
    let last = after.last().expect("the worker made a request");
    assert!(
        last.contains(&"shell".to_string()),
        "the worker's last request was prepared with the covering grant and must offer shell: {last:?} \
         (all: {after:?})"
    );
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn leader_approves_worker_shell_and_delegates_capabilities_end_to_end() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let leader = vec![
        Step::Message(json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": "spawn",
             "arguments": json!({"instance_id": "i-worker", "instructions": "run shell proof", "workspace": "isolated"}).to_string()}},
            {"id": "c2", "type": "function", "function": {"name": "delegate",
             "arguments": json!({"assignee": "i-worker", "task_id": "t-shell", "description": "write worker_proof.txt",
                                 "capabilities": ["shell@workspace"]}).to_string()}}
        ]})),
        Step::Message(wait_call("c3", json!({"mode": "ANY", "conditions": [{"kind": "task", "task_id": "t-shell"}]}))),
        Step::Message(json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "c4", "type": "function", "function": {"name": "decide_approval",
             "arguments": json!({"action": "list"}).to_string()}}
        ]})),
        Step::Dynamic(Box::new(|request: &ModelRequest| {
            let approval_id = request
                .messages
                .iter()
                .rev()
                .filter(|msg| msg["role"] == "tool")
                .filter_map(|msg| msg["content"].as_str())
                .filter_map(|content| serde_json::from_str::<Json>(content).ok())
                .find_map(|body| {
                    body["approvals"]
                        .as_array()
                        .and_then(|rows| rows.first().and_then(|row| row["approval_id"].as_str()).map(str::to_string))
                })
                .expect("decide_approval list returned pending worker approval");
            assert!(approval_id.starts_with("ap-"), "{approval_id}");
            json!({"role": "assistant", "content": "", "tool_calls": [
                {"id": "c5", "type": "function", "function": {"name": "decide_approval",
                 "arguments": json!({"action": "approve", "approval_id": approval_id}).to_string()}}
            ]})
        })),
        Step::Message(wait_call("c6", json!({"mode": "ANY", "conditions": [{"kind": "task", "task_id": "t-shell"}]}))),
        Step::Message(finish_call("approved worker shell and completed")),
    ];
    let worker = vec![
        Step::Message(json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "w1", "type": "function", "function": {"name": "shell",
             "arguments": json!({"command": "echo approved-by-leader > worker_proof.txt"}).to_string()}}
        ]})),
        Step::Message(finish_call("shell ran after leader approval")),
    ];
    let seen: Seen = Arc::new(Mutex::new(HashMap::new()));
    let root = root("leader-approves-worker");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let mut cfg = config(
        &root,
        factory_with_log(
            HashMap::from([("i-leader".to_string(), leader), ("i-worker".to_string(), worker)]),
            seen.clone(),
        ),
    );
    cfg.require_shell_approval = true;
    let handle = start(cfg).await.expect("start");
    handle.input("i-leader", "have worker run the shell command").await.expect("input");
    let closed = wait_event(&handle, "work_completed", 25_000).await;
    let proof = root.dir.join("state/instances/i-worker/work/worker_proof.txt");
    let proof_text = std::fs::read_to_string(&proof);
    let control = second_control(&root);
    let task_status: String =
        control.connection().query_row("SELECT status FROM tasks WHERE id = 't-shell'", [], |r| r.get(0)).unwrap();
    let op_row: (String, Option<String>) = control
        .connection()
        .query_row(
            "SELECT o.status, o.receipt_json FROM operations o
             JOIN decisions d ON d.decision_id = o.decision_id
             JOIN model_requests r ON r.request_id = d.request_id
             WHERE r.instance_id = 'i-worker' AND json_extract(o.intent_json, '$.name') = 'shell'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let granted_by_leader: i64 = control
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM events WHERE kind = 'approval_granted' AND json_extract(payload_json, '$.approver') = 'i-leader'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    drop(control);
    handle.shutdown().await.expect("shutdown");

    assert_eq!(closed["payload"]["status"], json!("SUCCEEDED"), "{closed}");
    assert_eq!(task_status, "SUCCEEDED");
    assert_eq!(op_row.0, "SUCCEEDED", "worker shell op receipt: {:?}", op_row.1);
    let text = proof_text.unwrap_or_else(|e| panic!("worker proof exists ({e}); shell op: {op_row:?}"));
    assert_eq!(text.trim(), "approved-by-leader");
    assert_eq!(granted_by_leader, 1);

    let log = seen.lock().unwrap();
    let leader_tools = log.get("i-leader").and_then(|reqs| reqs.first()).cloned().unwrap_or_default();
    let worker_tools = log.get("i-worker").and_then(|reqs| reqs.first()).cloned().unwrap_or_default();
    assert!(leader_tools.contains(&"decide_approval".to_string()), "{leader_tools:?}");
    assert!(!worker_tools.contains(&"decide_approval".to_string()), "{worker_tools:?}");
}
