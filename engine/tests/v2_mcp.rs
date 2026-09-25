//! R2-P4 R18: MCP services and skills enter the v2 kernel through the same
//! receipt contract as the built-in tools (plan §7) — advertised with each
//! model request (§5.2), executed via the member's bound tool set, recovered
//! honestly across a crash (A25/§6.3: a committed MCP dispatch is never
//! re-issued blindly, because the remote effect cannot be verified).

use serde_json::{json, Value as Json};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use teamagents_core::kernel::{KernelProfile, ModelRequest, ModelResponse, Usage};
use teamagents_core::models::{ToolBinding, UserConfig};
use teamagents_engine::providers::{AttemptOutcome, Cancel, Provider, ProviderError, ProviderEvent};
use teamagents_engine::reference::basic_tool_schemas;
use teamagents_engine::v2::driver::{start, DriverConfig, DriverHandle};

/// Tool names advertised with each model request, in request order.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<Vec<String>>>>);

struct ScriptedProvider {
    script: Mutex<VecDeque<Json>>,
    seen: Seen,
}

impl ScriptedProvider {
    fn new(script: Vec<Json>) -> ScriptedProvider {
        ScriptedProvider { script: Mutex::new(script.into()), seen: Seen::default() }
    }
    fn seen(&self) -> Seen {
        self.seen.clone()
    }
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
        let names = request
            .tools
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
            .collect::<Vec<_>>();
        self.seen.0.lock().unwrap().push(names);
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

fn tool_call(id: &str, name: &str, args: Json) -> Json {
    json!({"role": "assistant", "content": "",
           "tool_calls": [{"id": id, "type": "function",
                           "function": {"name": name, "arguments": args.to_string()}}]})
}

fn finish_call(summary: &str) -> Json {
    tool_call("finish-1", "finish", json!({"status": "success", "summary": summary}))
}

struct Root {
    dir: PathBuf,
}

fn root(tag: &str) -> Root {
    let dir = std::env::temp_dir().join(format!("teamagents-v2-mcp-{tag}-{}", uuid::Uuid::new_v4()));
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
            instances_dir: self.dir.join("state").join("instances"),
            workspace: self.dir.join("ws"),
            permissions: "full_auto".into(),
            profile: KernelProfile {
                model: "scripted".into(),
                instructions: "test agent".into(),
                tools: vec![],
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

async fn run_to_goal_close(handle: &DriverHandle) -> String {
    let event = wait_event(handle, "goal_completed", 15_000).await;
    event["payload"]["status"].as_str().unwrap_or("").to_string()
}

/// Second control connection over the same session DB (WAL multi-connection).
fn second_control(root: &Root) -> teamagents_core::v2::Control {
    teamagents_core::v2::Control::open(&root.dir.join("session.sqlite"), "s-test", true).expect("control")
}

/// tool_result context entries of the default instance, in context order.
fn tool_results(root: &Root) -> Vec<String> {
    let control = second_control(root);
    let rows = control
        .connection()
        .prepare("SELECT message_json FROM context_entries WHERE instance_id = 'i-main' AND kind = 'tool_result' ORDER BY idx")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    rows
}

/// Catalog with one stdio MCP service `echo_service` serving `echo_echo`.
/// Host mode on purpose: these cases cover binding/advertising/calling a real
/// stdio server, not the bwrap workspace (same choice as mcp_tools.rs).
fn echo_catalog(command: &str) -> UserConfig {
    let mut catalog = UserConfig::default();
    catalog.tools.insert(
        "echo_service".into(),
        serde_json::from_value::<ToolBinding>(json!({
            "kind": "mcp", "mcp_server": "echo", "mcp_transport": "stdio",
            "mcp_execution": "host",
            "command": command, "tool_names": ["echo"],
        }))
        .unwrap(),
    );
    catalog
}

#[tokio::test]
async fn mcp_tool_round_trips_through_the_same_receipt_contract() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("e2e");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let provider = ScriptedProvider::new(vec![
        tool_call("c1", "echo_echo", json!({"text": "ping", "times": 2})),
        finish_call("echoed"),
    ]);
    let seen = provider.seen();
    let mut config = root.config(provider);
    config.catalog = echo_catalog(env!("CARGO_BIN_EXE_fake-mcp-server"));
    config.bindings = vec!["echo_service".into()];
    let handle = start(config).await.expect("start");
    handle.input("echo twice").await.expect("input");
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    // advertised with the first model request already (§5.2)
    let advertised = seen.0.lock().unwrap().clone();
    assert!(advertised[0].iter().any(|name| name == "echo_echo"), "advertised: {advertised:?}");
    // executed through the bound set; the receipt landed as a context entry
    let results = tool_results(&root);
    assert!(results.iter().any(|body| body.contains("ping ping")), "{results:?}");
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn required_mcp_service_failure_fails_driver_boot() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("required");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let mut catalog = echo_catalog("/nonexistent/teamagents-mcp-server");
    catalog.tools.get_mut("echo_service").unwrap().required = true;
    let mut config = root.config(ScriptedProvider::new(vec![]));
    config.catalog = catalog;
    config.bindings = vec!["echo_service".into()];
    let error = start(config).await.err().expect("boot must fail");
    assert!(error.contains("echo_service"), "{error}");
}

#[tokio::test]
async fn optional_mcp_service_failure_only_drops_the_capability() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("optional");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let provider = ScriptedProvider::new(vec![finish_call("no echo needed")]);
    let seen = provider.seen();
    let mut config = root.config(provider);
    config.catalog = echo_catalog("/nonexistent/teamagents-mcp-server"); // optional by default
    config.bindings = vec!["echo_service".into()];
    let handle = start(config).await.expect("start");
    handle.input("just finish").await.expect("input");
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    let advertised = seen.0.lock().unwrap().clone();
    assert!(!advertised[0].iter().any(|name| name == "echo_echo"), "advertised: {advertised:?}");
    handle.shutdown().await.expect("shutdown");
}

/// A25/§6.3: a crash between DISPATCH_COMMITTED and the reply leaves the
/// remote outcome unverifiable — recovery marks it OUTCOME_UNKNOWN instead of
/// replaying the call against a fresh server process.
#[tokio::test]
async fn recovered_mcp_dispatch_is_not_replayed() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("crash");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let marker = root.dir.join("calls.txt");
    // a server that records the call, then blocks past the client's tool
    // timeout; the driver crash — never the server — ends the attempt
    let server = root.dir.join("block_server.py");
    std::fs::write(
        &server,
        r#"
import json, sys, time
marker = sys.argv[1]
for line in sys.stdin:
    try:
        request = json.loads(line)
    except Exception:
        continue
    if 'id' not in request:
        continue
    method = request.get('method', '')
    if method == 'initialize':
        result = {'protocolVersion': '2025-06-18', 'capabilities': {'tools': {}},
                  'serverInfo': {'name': 'block', 'version': '0'}}
    elif method == 'tools/list':
        result = {'tools': [{'name': 'echo', 'inputSchema': {'type': 'object'}}]}
    elif method == 'tools/call':
        with open(marker, 'a') as f:
            f.write('called\n')
        time.sleep(10)
        result = {'content': [{'type': 'text', 'text': 'late'}]}
    else:
        result = {}
    sys.stdout.write(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}) + '\n')
    sys.stdout.flush()
"#,
    )
    .unwrap();
    let mut catalog = UserConfig::default();
    catalog.tools.insert(
        "echo_service".into(),
        serde_json::from_value::<ToolBinding>(json!({
            "kind": "mcp", "mcp_server": "echo", "mcp_transport": "stdio",
            "mcp_execution": "host", "tool_timeout_s": 3,
            "command": "python3", "args": [server.to_string_lossy(), marker.to_string_lossy()],
            "tool_names": ["echo"],
        }))
        .unwrap(),
    );
    let provider = ScriptedProvider::new(vec![tool_call("c1", "echo_echo", json!({"text": "ping"}))]);
    let mut config = root.config(provider);
    config.catalog = catalog.clone();
    config.bindings = vec!["echo_service".into()];
    let handle = start(config).await.expect("start");
    handle.input("echo once").await.expect("input");
    // crash right after the dispatch is committed (§6.3 crash window)
    wait_event(&handle, "operation_dispatched", 10_000).await;
    handle.crash().await;
    let recovering = ScriptedProvider::new(vec![finish_call("recovered without guessing")]);
    let mut config = root.config(recovering);
    config.catalog = catalog;
    config.bindings = vec!["echo_service".into()];
    let handle = start(config).await.expect("restart");
    let event = wait_event(&handle, "operation_completed", 10_000).await;
    assert_eq!(event["payload"]["status"], json!("OUTCOME_UNKNOWN"));
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    let calls = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(calls.matches("called").count(), 1, "the committed MCP call must not be re-issued");
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn skill_tool_searches_and_reads_through_the_same_contract() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("skills");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let registry = root.dir.join("registry");
    std::fs::create_dir_all(registry.join("ponytail")).unwrap();
    std::fs::write(
        registry.join("ponytail/SKILL.md"),
        "---\nname: ponytail\ndescription: laziest solution that works\n---\nponytail body",
    )
    .unwrap();
    let provider = ScriptedProvider::new(vec![
        tool_call("c1", "skill", json!({"action": "search", "query": "laziest"})),
        tool_call("c2", "skill", json!({"action": "read", "name": "ponytail"})),
        finish_call("skill loaded"),
    ]);
    let seen = provider.seen();
    let mut config = root.config(provider);
    config.profile.tools = basic_tool_schemas(false, true);
    config.catalog.skills_paths = vec![registry.to_string_lossy().into_owned()];
    config.bindings = vec!["skills".into()];
    let handle = start(config).await.expect("start");
    handle.input("use a skill").await.expect("input");
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    let advertised = seen.0.lock().unwrap().clone();
    assert!(advertised[0].iter().any(|name| name == "skill"), "advertised: {advertised:?}");
    let results = tool_results(&root);
    assert!(results.iter().any(|body| body.contains("ponytail —")), "{results:?}");
    assert!(results.iter().any(|body| body.contains("ponytail body")), "{results:?}");
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn skill_call_without_the_binding_fails_honestly() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    let root = root("skills-unbound");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let provider = ScriptedProvider::new(vec![
        tool_call("c1", "skill", json!({"action": "search", "query": "laziest"})),
        finish_call("reported the failure"),
    ]);
    let mut config = root.config(provider);
    config.profile.tools = basic_tool_schemas(false, true);
    config.bindings = vec![]; // advertised by the profile, but not bound
    let handle = start(config).await.expect("start");
    handle.input("use a skill").await.expect("input");
    assert_eq!(run_to_goal_close(&handle).await, "SUCCEEDED");
    let results = tool_results(&root);
    assert!(results.iter().any(|body| body.contains("not bound")), "{results:?}");
    handle.shutdown().await.expect("shutdown");
}
