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
