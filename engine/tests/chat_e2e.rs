//! End-to-end chat member tests: a fake OpenAI HTTP service + the real
//! ChatRunner driven through the real core and runtime (review F-6).
//!
//! Covers: once-approval consumption and EXPIRED handling (F-3/F-12), denial,
//! the model-step budget (F-1), active-timeout interruption (F-2/F-9),
//! conversation persistence across a restart (F-4), HTTP retry policy (F-11)
//! and the reasoning-effort fallback (F-7).

mod support;
use support::isolated_state_home as env_guard;

use serde_json::{json, Value as Json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use support::{core_with_spec, submit, wait_for};
use teamagents_core::models::{ModelProfile, TurnRun, TurnStatus, UserConfig};
use teamagents_engine::bound::BoundTools;
use teamagents_engine::chat::ChatRunner;
use teamagents_engine::core_client::CoreClient;
use teamagents_engine::gateway::{ApprovalGate, PermissionPolicy, ToolGateway};
use teamagents_engine::runtime::{AgentRunner, Notify, Runtime, RuntimeLimits, ToolExecutor};

// ---- fake OpenAI service ---------------------------------------------------

struct FakeOpenAi {
    port: u16,
    bodies: Arc<Mutex<Vec<Json>>>,
    calls: Arc<AtomicUsize>,
}

impl FakeOpenAi {
    fn start(handler: impl Fn(&Json, usize) -> (u16, Json) + Send + Sync + 'static) -> FakeOpenAi {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake openai");
        let port = listener.local_addr().unwrap().port();
        let bodies: Arc<Mutex<Vec<Json>>> = Arc::new(Mutex::new(vec![]));
        let calls = Arc::new(AtomicUsize::new(0));
        let (recorded, counter) = (bodies.clone(), calls.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let raw = read_request(&mut stream);
                let request: Json = serde_json::from_str(&raw).unwrap_or(Json::Null);
                let index = counter.fetch_add(1, Ordering::SeqCst);
                recorded.lock().unwrap().push(request.clone());
                let (status, response) = handler(&request, index);
                let payload = response.to_string();
                let head = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    payload.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(payload.as_bytes());
                let _ = stream.flush();
            }
        });
        FakeOpenAi { port, bodies, calls }
    }

    /// Same server, but every reply is an SSE body — the shape a real
    /// `stream: true` endpoint (chat completions, Messages, Responses) returns.
    fn start_sse(handler: impl Fn(&Json, usize) -> String + Send + Sync + 'static) -> FakeOpenAi {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake openai");
        let port = listener.local_addr().unwrap().port();
        let bodies: Arc<Mutex<Vec<Json>>> = Arc::new(Mutex::new(vec![]));
        let calls = Arc::new(AtomicUsize::new(0));
        let (recorded, counter) = (bodies.clone(), calls.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let raw = read_request(&mut stream);
                let request: Json = serde_json::from_str(&raw).unwrap_or(Json::Null);
                let index = counter.fetch_add(1, Ordering::SeqCst);
                recorded.lock().unwrap().push(request.clone());
                let payload = handler(&request, index);
                let head = format!(
                    "HTTP/1.1 200 X\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    payload.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(payload.as_bytes());
                let _ = stream.flush();
            }
        });
        FakeOpenAi { port, bodies, calls }
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn body(&self, index: usize) -> Json {
        self.bodies.lock().unwrap().get(index).cloned().unwrap_or(Json::Null)
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut buf: Vec<u8> = vec![];
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            return String::new();
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            if key.eq_ignore_ascii_case("content-length") {
                value.trim().parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    while buf.len() < header_end + length {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    String::from_utf8_lossy(&buf[header_end..(header_end + length).min(buf.len())]).to_string()
}

fn tool_call_response(call_id: &str, tool: &str, args: Json) -> Json {
    json!({"id": "x", "choices": [{"message": {
        "role": "assistant", "content": Json::Null,
        "tool_calls": [{"id": call_id, "type": "function",
                        "function": {"name": tool, "arguments": args.to_string()}}]}}]})
}

fn text_response(text: &str) -> Json {
    json!({"id": "x", "choices": [{"message": {"role": "assistant", "content": text}}]})
}

#[test]
#[ignore = "v1 后端随 R29 退役（teamagents exec 现为 v2 无头客户端）；等价覆盖：v2_driver::user_cancel_stops_a_running_job、jobs_runner::a_successful_commands_service_outlives_the_job（A12/D-41）、v2_driver 崩溃恢复组"]
fn exec_full_auto_service_survives_cli_exit_and_checks_use_host_environment() {
    let env = env_guard("exec-host-service");
    let project = env.join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(env.join("host-only"), "host evidence").unwrap();
    std::fs::write(
        project.join("server.py"),
        r#"from http.server import HTTPServer, BaseHTTPRequestHandler
from pathlib import Path
class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'service-survived')
server = HTTPServer(('127.0.0.1', 0), Handler)
Path('port').write_text(str(server.server_port))
server.serve_forever()
"#,
    )
    .unwrap();
    struct Service(std::path::PathBuf);
    impl Drop for Service {
        fn drop(&mut self) {
            if let Ok(pid) = std::fs::read_to_string(self.0.join("service.pid")) {
                let _ = std::process::Command::new("/bin/kill").args(["-KILL", pid.trim()]).status();
            }
        }
    }
    let _service = Service(project.clone());
    let check = "test -r ../host-only && python3 -c \"import urllib.request; print(urllib.request.urlopen('http://127.0.0.1:'+open('port').read(), timeout=2).read().decode())\"";
    let api = FakeOpenAi::start(move |_, index| {
        (
            200,
            match index {
                0 => tool_call_response(
                    "start",
                    "shell",
                    json!({"command":
            "python3 server.py >service.log 2>&1 </dev/null & echo $! >service.pid; for i in {1..100}; do test -s port && break; sleep 0.02; done; test -s port"}),
                ),
                1 => tool_call_response("probe", "shell", json!({"command": check})),
                2 => tool_call_response("done", "signal_done", json!({"summary":"Separate-call HTTP probe passed"})),
                _ => text_response("Service ready"),
            },
        )
    });
    let config = teamagents_engine::config::user_config_path();
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        config,
        format!("[models.leader_main]\nprovider='openai'\nmodel='test'\nbase_url='{}'\n", api.base_url()),
    )
    .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_teamagents"))
        .args(["exec", "--json", "--full-auto", "--timeout", "10", "--cwd"])
        .arg(&project)
        .args(["--check", check, "Start and verify the local service"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: Vec<Json> =
        String::from_utf8_lossy(&output.stdout).lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let result = rows.iter().find(|row| row["type"] == "result").unwrap();
    assert_eq!(result["verification"][0]["ok"], true, "{result}");
    assert!(result["verification"][0]["output"].as_str().unwrap().contains("service-survived"));
    let port = std::fs::read_to_string(project.join("port")).unwrap();
    assert_eq!(
        ureq::get(&format!("http://127.0.0.1:{port}/")).call().unwrap().into_string().unwrap(),
        "service-survived"
    );
    let prompt = api.body(0)["messages"][0]["content"].as_str().unwrap().to_string();
    assert!(prompt.contains("<shell_environment>full_auto"), "{prompt}");
}

#[test]
fn shell_execution_and_prompt_follow_live_mode_changes_with_cached_executor() {
    if !teamagents_engine::tools::bwrap_available() {
        return;
    }
    let env = env_guard("shell-mode-switch");
    let project = env.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let outside = env.join("host-only");
    std::fs::write(&outside, "host").unwrap();
    let command = format!("if test -r '{}'; then echo HOST_VISIBLE; else echo HOST_HIDDEN; fi", outside.display());
    let api = FakeOpenAi::start(move |_, index| {
        (
            200,
            if index % 2 == 0 {
                // Model-supplied mode fields cannot select host execution.
                tool_call_response(
                    &format!("probe-{index}"),
                    "shell",
                    json!({"command":command,"mode":"full_auto","full_auto":true}),
                )
            } else {
                text_response("checked")
            },
        )
    });
    let mut catalog = UserConfig::default();
    catalog.models.insert("m".into(), profile(&api.base_url(), "openai", json!({}), 0));
    let opened = open_session(OpenOptions {
        cwd: Some(project),
        session_id: Some("shell-mode-switch".into()),
        catalog: Some(catalog),
        full_auto: true,
        initial_spec: Some(json!({"leader_id":"leader","agents":[agent_json("leader","leader", &["files","shell"])]})),
        ..Default::default()
    })
    .unwrap();
    opened.runtime.start();
    for (i, mode, marker) in
        [(0, "full_auto", "HOST_VISIBLE"), (1, "approved_scope", "HOST_HIDDEN"), (2, "full_auto", "HOST_VISIBLE")]
    {
        assert!(submit(&opened.core, &format!("mode-{i}"), "user", "set_permission_mode", json!({"mode":mode})).ok);
        opened.runtime.user_message("Probe current mode", false).unwrap();
        assert!(opened.runtime.settle(5));
        let body = api.body(i * 2 + 1);
        let messages = body["messages"].as_array().unwrap();
        let output =
            messages.iter().rev().find(|message| message["role"] == "tool").unwrap()["content"].as_str().unwrap();
        assert!(output.contains(marker), "{output}");
        assert!(messages[0]["content"].as_str().unwrap().contains(&format!("<shell_environment>{mode}")));
    }
    opened.close();
}

fn private_helper_request(body: &Json) -> bool {
    body["messages"]
        .as_array()
        .and_then(|messages| messages.first())
        .and_then(|message| message["content"].as_str())
        .is_some_and(|content| content.contains("<teamagents_private_subagent>"))
}

fn body_has_role(body: &Json, role: &str) -> bool {
    body["messages"].as_array().is_some_and(|messages| messages.iter().any(|message| message["role"] == role))
}

// ---- harness ---------------------------------------------------------------

fn agent_json(id: &str, role: &str, bindings: &[&str]) -> Json {
    json!({"id": id, "name": id, "role": role, "runtime_kind": "deepagents",
           "model_profile": "m", "tool_bindings": bindings})
}

fn profile(base_url: &str, protocol: &str, options: Json, max_retries: i64) -> ModelProfile {
    ModelProfile {
        provider: "openai".into(),
        protocol: protocol.into(),
        model: "test".into(),
        base_url: Some(base_url.to_string()),
        api_key_env: None,
        timeout: 30,
        max_retries,
        generation_options: serde_json::from_value(options).unwrap_or_default(),
        context_window: None,
        codex_profile: None,
    }
}

fn chat_runner(core: &Arc<CoreClient>, agent: &Json, profile: ModelProfile, workdir: &str) -> Arc<ChatRunner> {
    chat_runner_with(agent, profile, workdir, Notify::new(core.clone()))
}

#[test]
fn stale_views_and_buffered_inbox_are_rechecked_before_chat_history() {
    let _env = env_guard("chat-delivery-acl");
    for buffered in [false, true] {
        for revoke in [false, true] {
            let session = format!("chat-delivery-{buffered}-{revoke}");
            let agent = agent_json("watch", "worker", &[]);
            let mut spec = json!({
                "leader_id": "leader",
                "agents": [agent_json("leader", "leader", &[]), agent_json("worker", "worker", &[]), agent],
                "observers": [{
                    "agent_id": "watch", "subjects": ["worker"], "event_types": ["task_completed"],
                    "payload_scope": "result", "wake_policy": "on_event"
                }]
            });
            let core = core_with_spec(&session, spec.clone());
            core.call_in_session(
                "emit",
                json!({"actor_id": "worker", "events": [{
                    "kind": "task_completed", "payload": {
                        "task_id": "private-task", "assignee": "worker", "requester": "leader",
                        "status": "SUCCEEDED", "result_refs": ["PRIVATE-REF"], "summary": "PRIVATE SUMMARY"
                    }
                }]}),
            )
            .unwrap();
            let mut view = core.call_in_session("agent_view", json!({"agent_id": "watch"})).unwrap();
            assert!(view.to_string().contains("PRIVATE-REF"));
            let offered_ids: Vec<i64> = serde_json::from_value(view["delivery_ids"].clone()).unwrap();
            let run: TurnRun = serde_json::from_value(
                core.state_brief().unwrap()["runs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|r| r["agent_id"] == "watch")
                    .unwrap()
                    .clone(),
            )
            .unwrap();
            let server = FakeOpenAi::start(|_, _| (200, text_response("received authorized input")));
            let runner = chat_runner(&core, &agent, profile(&server.base_url(), "openai", json!({}), 0), "/tmp");
            if buffered {
                runner.deliver_mid_turn(&run.run_id, view["inbox_delta"].as_array().unwrap().clone());
                view["inbox_delta"] = json!([]);
                view["delivery_ids"] = json!([]);
            }
            if revoke {
                spec["observers"] = json!([]);
            } else {
                spec["observers"][0]["payload_scope"] = json!("status");
            }
            core.call_in_session("save_spec", json!({"spec": spec})).unwrap();
            let gate = ToolGateway::new(
                core.clone(),
                "watch",
                &run.run_id,
                ApprovalGate::new(core.clone(), PermissionPolicy::default()),
                None,
            );
            let outcome = runner.start_or_resume(&run, &view, &gate, &Json::Null);
            assert_eq!(outcome.status, TurnStatus::Completed, "{outcome:?}");
            let wire = server.body(0).to_string();
            assert!(!wire.contains("PRIVATE-REF") && !wire.contains("PRIVATE SUMMARY"), "{wire}");
            assert_eq!(wire.contains("private-task"), !revoke);
            assert_eq!(runner.applied_delivery_ids(&run).unwrap(), if revoke { vec![] } else { offered_ids });
            runner.close();
        }
    }
}

fn chat_runner_with(agent: &Json, profile: ModelProfile, workdir: &str, notify: Arc<Notify>) -> Arc<ChatRunner> {
    let bindings: Vec<String> = agent
        .get("tool_bindings")
        .and_then(|v| v.as_array())
        .map(|items| items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let bound = BoundTools::load(&UserConfig::default(), &bindings).expect("bound tools");
    ChatRunner::new(agent, profile, Some(workdir.to_string()), notify, bound, vec![], (false, false))
}

fn start_runtime(
    core: &Arc<CoreClient>,
    runner: Arc<ChatRunner>,
    agent_id: &str,
    policy: PermissionPolicy,
    limits: RuntimeLimits,
    executor: ToolExecutor,
) -> Arc<Runtime> {
    start_runtime_with(core, runner, agent_id, policy, limits, executor, Notify::new(core.clone()))
}

fn start_runtime_with(
    core: &Arc<CoreClient>,
    runner: Arc<ChatRunner>,
    agent_id: &str,
    policy: PermissionPolicy,
    limits: RuntimeLimits,
    executor: ToolExecutor,
    notify: Arc<Notify>,
) -> Arc<Runtime> {
    let approvals = ApprovalGate::new(core.clone(), policy);
    let runtime = Runtime::new(core.clone(), notify, approvals, executor, None, limits);
    runtime.add_runner(agent_id, runner);
    runtime.start();
    runtime
}

type RecordedCalls = Arc<Mutex<Vec<(String, Json)>>>;

fn recording_executor() -> (ToolExecutor, RecordedCalls) {
    let calls: Arc<Mutex<Vec<(String, Json)>>> = Arc::new(Mutex::new(vec![]));
    let sink = calls.clone();
    let executor: ToolExecutor =
        Arc::new(move |_agent: &str, tool: &str, args: &Json, _control: &teamagents_engine::gateway::TurnControl| {
            sink.lock().unwrap().push((tool.to_string(), args.clone()));
            Ok(json!({"output": "executed"}))
        });
    (executor, calls)
}

fn approval_policy_for_shell() -> PermissionPolicy {
    PermissionPolicy {
        mode: "approved_scope".into(),
        pre_authorized: ["files"].iter().map(|s| s.to_string()).collect(),
        require_approval: ["shell"].iter().map(|s| s.to_string()).collect(),
    }
}

fn pending_approvals(core: &Arc<CoreClient>) -> Vec<Json> {
    core.state()
        .ok()
        .and_then(|state| state.get("pending_approvals").and_then(|v| v.as_array()).cloned())
        .unwrap_or_default()
}

fn runs(core: &Arc<CoreClient>) -> Vec<TurnRun> {
    core.state()
        .ok()
        .and_then(|state| serde_json::from_value::<Vec<TurnRun>>(state.get("runs").cloned().unwrap_or(Json::Null)).ok())
        .unwrap_or_default()
}

fn event_kinds(core: &Arc<CoreClient>) -> Vec<String> {
    core.state()
        .ok()
        .and_then(|state| state.get("events").and_then(|v| v.as_array()).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|e| e.get("kind").and_then(|v| v.as_str()).map(str::to_string))
        .collect()
}

fn approval_status(core: &Arc<CoreClient>, approval_id: &str) -> String {
    core.call_in_session("get_approval", json!({"approval_id": approval_id}))
        .ok()
        .and_then(|reply| {
            reply.get("approval").and_then(|a| a.get("status")).and_then(|v| v.as_str()).map(str::to_string)
        })
        .unwrap_or_default()
}

fn shell_args() -> Json {
    json!({"command": "echo hi"})
}

/// A member whose turn thread panics (F-9).
struct PanicRunner;

impl AgentRunner for PanicRunner {
    fn start_or_resume(
        &self,
        _run: &TurnRun,
        _view: &Json,
        _gateway: &ToolGateway,
        _wake: &Json,
    ) -> teamagents_core::control::TurnOutcome {
        panic!("boom");
    }
    fn request_interrupt(&self, _run_id: &str) -> TurnStatus {
        TurnStatus::Cancelled
    }
    fn query_state(&self, _run_id: &str) -> Option<TurnStatus> {
        None
    }
    fn deliver_mid_turn(&self, _run_id: &str, _items: Vec<Json>) {}
}

fn tool_call_with_usage(call_id: &str, tool: &str, args: Json, prompt: u64) -> Json {
    let mut response = tool_call_response(call_id, tool, args);
    response["usage"] = json!({"prompt_tokens": prompt, "completion_tokens": 10, "total_tokens": prompt + 10});
    response
}

fn text_with_usage(text: &str, prompt: u64) -> Json {
    let mut response = text_response(text);
    response["usage"] = json!({"prompt_tokens": prompt, "completion_tokens": 10, "total_tokens": prompt + 10});
    response
}

// ---- tests -----------------------------------------------------------------

/// The environment is a system-level field on every supported wire protocol,
/// including an empty member config and a restart with updated role instructions.
#[test]
fn worker_environment_reaches_model_before_leader_task_on_all_protocols() {
    let _env = env_guard("worker-system-prompt");
    for protocol in ["openai", "anthropic", "responses"] {
        let server = FakeOpenAi::start(move |_, _| {
            (
                200,
                match protocol {
                    "anthropic" => json!({"id":"msg-1", "type":"message", "role":"assistant", "stop_reason":"end_turn",
                "content":[{"type":"text", "text":"worker reply"}]}),
                    "responses" => {
                        json!({"id":"resp-1", "status":"completed", "output":[{"type":"message", "role":"assistant",
                "content":[{"type":"output_text", "text":"worker reply"}]}]})
                    }
                    _ => text_response("worker reply"),
                },
            )
        });
        let session = format!("worker-prompt-{protocol}");
        let mut agent = agent_json("worker", "worker", &[]);
        let core = core_with_spec(
            &session,
            json!({"leader_id":"leader",
            "agents":[agent_json("leader", "leader", &[]), agent.clone()]}),
        );
        let view = json!({"agent_id":"worker", "assignment":[{
            "task_id":"task-1", "description":"Review the parser", "acceptance":"Cite test evidence", "requester":"leader"}],
            "inbox_delta":[], "permitted_shared_delta":[], "relevant_topology":{"revision":1}});
        for (index, instructions) in ["", "Focus on parser edge cases."].into_iter().enumerate() {
            agent["instructions"] = json!(instructions);
            let runner = chat_runner(&core, &agent, profile(&server.base_url(), protocol, json!({}), 0), "/tmp");
            let run_id = format!("worker-run-{index}");
            let run: TurnRun = serde_json::from_value(json!({
                "run_id":run_id, "session_id":session, "agent_id":"worker", "task_id":"task-1", "goal_id":null,
                "status":"QUEUED", "config_revision":1, "topology_revision":1, "input_delivery_ids":[],
                "context_ref":"ctx:worker", "external_turn_id":null, "cancel_requested":false,
                "waiting_on":[], "created_at":0, "updated_at":0,
            }))
            .unwrap();
            let gateway = ToolGateway::new(
                core.clone(),
                "worker",
                &run_id,
                ApprovalGate::new(core.clone(), PermissionPolicy::default()),
                None,
            );
            let outcome = runner.start_or_resume(&run, &view, &gateway, &json!({"reason":"new_input"}));
            assert_eq!(outcome.status, TurnStatus::Completed, "{protocol}: {outcome:?}");
            let body = server.body(index);
            let (system, input) = match protocol {
                "anthropic" => (body["system"].as_str().unwrap(), body["messages"].to_string()),
                "responses" => (body["instructions"].as_str().unwrap(), body["input"].to_string()),
                _ => {
                    assert_eq!(body["messages"][0]["role"], "system");
                    assert_eq!(
                        body["messages"].as_array().unwrap().iter().filter(|m| m["role"] == "system").count(),
                        1
                    );
                    (body["messages"][0]["content"].as_str().unwrap(), body["messages"][1].to_string())
                }
            };
            assert!(system.starts_with("<teamagents_worker>"), "{protocol}: {body}");
            assert!(system.contains("acceptance criteria") && system.contains("complete_task"));
            assert!(!system.contains("Review the parser"), "task input must stay separate from the environment");
            assert!(input.contains("Review the parser"), "{protocol}: {body}");
            if !instructions.is_empty() {
                assert!(system.find("</teamagents_worker>").unwrap() < system.find(instructions).unwrap());
                assert!(body.to_string().contains("worker reply"), "the prior conversation survives refresh");
            }
            runner.close();
        }
    }
}

/// F-3: a once approval must be found again when the model re-sends the call
/// with a fresh tool_call_id, the operation must run, and the row must be
/// consumed (EXPIRED) — then the turn finishes.
/// The Responses wire format (`/responses`) must round-trip tool calls: the
/// engine keeps chat-completions history internally, so this checks both the
/// translation out (`function_call_output`) and back (`tool_calls`).
#[test]
fn responses_protocol_round_trips_a_tool_call() {
    let _env = env_guard("chat-responses");
    let sse =
        |frames: Vec<Json>| -> String { frames.iter().map(|frame| format!("event: x\ndata: {frame}\n\n")).collect() };
    let server = FakeOpenAi::start_sse(move |_body, index| {
        if index == 0 {
            sse(vec![
                json!({"type":"response.output_item.done","item":{"type":"function_call","call_id":"call-1","name":"shell","arguments":"{\"command\":\"echo hi\"}"}}),
                json!({"type":"response.completed","response":{"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15},
                    "output":[{"type":"function_call","call_id":"call-1","name":"shell","arguments":"{\"command\":\"echo hi\"}"}]}}),
            ])
        } else {
            sse(vec![
                json!({"type":"response.completed","response":{"usage":{"input_tokens":12,"output_tokens":3,"total_tokens":15},
                "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"all done"}]}]}}),
            ])
        }
    });
    let mut agent = agent_json("leader", "leader", &["shell"]);
    agent["instructions"] = json!("You are the Leader of a team of agents.");
    let spec = json!({"leader_id": "leader", "agents": [agent.clone()]});
    let core = core_with_spec("s-responses", spec);
    let mut model = profile(&server.base_url(), "responses", json!({}), 0);
    model.max_retries = 0;
    let runner = chat_runner(&core, &agent, model, "/tmp");
    let (executor, tool_calls) = recording_executor();
    // shell is pre-authorized here: this test is about the wire format, not approvals
    let runtime =
        start_runtime(&core, runner, "leader", PermissionPolicy::default(), RuntimeLimits::default(), executor);

    runtime.user_message("run echo", false).unwrap();
    assert!(wait_for(|| tool_calls.lock().unwrap().len() == 1, 15_000), "the tool call reached the executor");
    assert!(wait_for(|| !runs(&core).iter().any(|r| r.status.is_active()), 15_000), "the turn finished");

    assert_eq!(tool_calls.lock().unwrap()[0].1, shell_args());
    // the follow-up request carried the tool result back in Responses shape
    let second = server.body(1);
    let input = second["input"].as_array().cloned().unwrap_or_default();
    assert!(
        input.iter().any(|item| item["type"] == "function_call_output"
            && item["call_id"] == "call-1"
            && item["output"].as_str().unwrap_or("").contains("executed")),
        "tool output must travel as function_call_output: {second}"
    );
    assert!(
        second["instructions"].as_str().unwrap_or("").contains("Leader"),
        "system prompt maps to instructions: {second}"
    );
    let shell_tool =
        second["tools"].as_array().unwrap().iter().find(|tool| tool["name"] == "shell").cloned().unwrap_or(Json::Null);
    assert_eq!(shell_tool["type"], "function", "tools are flattened for Responses: {shell_tool}");
    assert!(shell_tool.get("function").is_none(), "no nested function object: {shell_tool}");
    assert_eq!(shell_tool["parameters"]["properties"]["command"]["type"], "string");
    assert!(second["stream"].as_bool().unwrap_or(false), "streaming stays on: {second}");
    runtime.close();
}

#[test]
fn incomplete_model_responses_never_execute_tools() {
    let _env = env_guard("incomplete-responses");
    for protocol in ["openai", "anthropic", "responses"] {
        for streaming in [false, true] {
            let call = json!({"type":"function_call", "call_id":"partial", "name":"shell", "arguments":shell_args().to_string()});
            let data = match protocol {
                "responses" => {
                    json!({"status":"incomplete", "incomplete_details":{"reason":"max_output_tokens"}, "output":[call]})
                }
                "anthropic" => {
                    json!({"stop_reason":"max_tokens", "content":[{"type":"tool_use", "id":"partial", "name":"shell", "input":shell_args()}]})
                }
                _ => {
                    let mut data = tool_call_response("partial", "shell", shell_args());
                    data["choices"][0]["finish_reason"] = json!("length");
                    data
                }
            };
            let server = if streaming {
                FakeOpenAi::start_sse(move |_, _| {
                    let frames = match protocol {
                        "responses" => vec![json!({"type":"response.incomplete", "response":data})],
                        "anthropic" => vec![
                            json!({"type":"content_block_start", "index":0, "content_block":data["content"][0]}),
                            json!({"type":"message_delta", "delta":{"stop_reason":"max_tokens"}}),
                            json!({"type":"message_stop"}),
                        ],
                        _ => vec![json!({"choices":[{"index":0,"finish_reason":"length", "delta":{"tool_calls":[{
                            "index":0,"id":"partial","type":"function","function":{"name":"shell","arguments":shell_args().to_string()}
                        }]}}]})],
                    };
                    let mut wire: String = frames.iter().map(|frame| format!("data: {frame}\n\n")).collect();
                    if protocol == "openai" {
                        wire.push_str("data: [DONE]\n\n");
                    }
                    wire
                })
            } else {
                FakeOpenAi::start(move |_, _| (200, data.clone()))
            };
            let agent = agent_json("leader", "leader", &["shell"]);
            let core = core_with_spec(
                &format!("incomplete-{protocol}-{streaming}"),
                json!({"leader_id":"leader","agents":[agent.clone()]}),
            );
            let runner = chat_runner(&core, &agent, profile(&server.base_url(), protocol, json!({}), 2), "/tmp");
            let (executor, calls) = recording_executor();
            let runtime =
                start_runtime(&core, runner, "leader", PermissionPolicy::default(), RuntimeLimits::default(), executor);
            runtime.user_message("run echo", false).unwrap();
            assert!(wait_for(|| runs(&core).iter().any(|r| !r.status.is_active()), 5_000));
            runtime.close();
            assert!(
                calls.lock().unwrap().is_empty(),
                "{protocol} streaming={streaming} executed an incomplete response"
            );
            assert_eq!(runs(&core)[0].status, TurnStatus::Failed);
            assert_eq!(server.calls(), 1, "an incomplete response must not be silently replayed");
        }
    }
}

/// Transport recovery: a truncated stream that never showed text is retried
/// within max_retries; the complete response on the retry executes exactly
/// once and the turn completes.
#[test]
fn truncated_tool_stream_retries_and_executes_once() {
    let _env = env_guard("stream-retry");
    let sse = |frames: Vec<Json>| -> String { frames.iter().map(|frame| format!("data: {frame}\n\n")).collect() };
    for protocol in ["openai", "anthropic", "responses"] {
        let args = shell_args().to_string();
        // Attempt 0 dies mid-stream before any visible output (tool-only so far).
        let truncated = match protocol {
            "anthropic" => sse(vec![
                json!({"type":"message_start","message":{"usage":{"input_tokens":3,"output_tokens":0}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"c1","name":"shell","input":{}}}),
            ]),
            "responses" => sse(vec![json!({"type":"response.created","response":{"id":"r1"}})]),
            _ => sse(vec![
                json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"shell","arguments":"{\"comma"}}]}}]}),
            ]),
        };
        let complete = match protocol {
            "anthropic" => sse(vec![
                json!({"type":"message_start","message":{"usage":{"input_tokens":5,"output_tokens":0}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"c1","name":"shell","input":{}}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":args}}),
                json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}}),
                json!({"type":"message_stop"}),
            ]),
            "responses" => sse(vec![
                json!({"type":"response.output_item.done","item":{"type":"function_call","call_id":"c1","name":"shell","arguments":args}}),
                json!({"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":5,"output_tokens":7,"total_tokens":12},
                    "output":[{"type":"function_call","call_id":"c1","name":"shell","arguments":args}]}}),
            ]),
            _ => {
                let mut wire = sse(vec![
                    json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"shell","arguments":args}}]}}]}),
                    json!({"choices":[{"finish_reason":"tool_calls","delta":{}}]}),
                ]);
                wire.push_str("data: [DONE]\n\n");
                wire
            }
        };
        let done = match protocol {
            "anthropic" => sse(vec![
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"done"}}),
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),
                json!({"type":"message_stop"}),
            ]),
            "responses" => sse(vec![json!({"type":"response.completed","response":{"status":"completed",
                "usage":{"input_tokens":6,"output_tokens":2,"total_tokens":8},
                "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}]}})]),
            _ => {
                let mut wire = sse(vec![
                    json!({"choices":[{"delta":{"content":"done"}}]}),
                    json!({"choices":[{"finish_reason":"stop","delta":{}}]}),
                ]);
                wire.push_str("data: [DONE]\n\n");
                wire
            }
        };
        let server = FakeOpenAi::start_sse(move |_, index| match index {
            0 => truncated.clone(),
            1 => complete.clone(),
            _ => done.clone(),
        });
        let agent = agent_json("leader", "leader", &["shell"]);
        let core =
            core_with_spec(&format!("stream-retry-{protocol}"), json!({"leader_id":"leader","agents":[agent.clone()]}));
        let runner = chat_runner(&core, &agent, profile(&server.base_url(), protocol, json!({}), 2), "/tmp");
        let (executor, calls) = recording_executor();
        // shell is pre-authorized here: this test is about transport recovery, not approvals
        let runtime =
            start_runtime(&core, runner, "leader", PermissionPolicy::default(), RuntimeLimits::default(), executor);
        runtime.user_message("run echo", false).unwrap();
        assert!(
            wait_for(|| runs(&core).iter().any(|r| !r.status.is_active()), 15_000),
            "{protocol}: the turn finished"
        );
        runtime.close();
        let executed = calls.lock().unwrap();
        assert_eq!(executed.len(), 1, "{protocol}: the tool executed exactly once");
        assert_eq!(executed[0].1, shell_args(), "{protocol}: full arguments reached the executor");
        drop(executed);
        assert_eq!(server.calls(), 3, "{protocol}: truncated attempt + retry + final answer");
        assert_eq!(runs(&core)[0].status, TurnStatus::Completed, "{protocol}: the retried turn completed");
    }
}

/// The retry is bounded by max_retries and never replays a stream that
/// already showed text to the user.
#[test]
fn stream_retry_respects_budget_and_visible_output() {
    let _env = env_guard("stream-retry-budget");
    let partial_tool = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"shell\",\"arguments\":\"{\\\"comma\"}}]}}]}\n\n".to_string();

    // No configured retries: the first truncated attempt ends the turn.
    let always_partial = partial_tool.clone();
    let server = FakeOpenAi::start_sse(move |_, _| always_partial.clone());
    let agent = agent_json("leader", "leader", &["shell"]);
    let core = core_with_spec("stream-budget-0", json!({"leader_id":"leader","agents":[agent.clone()]}));
    let runner = chat_runner(&core, &agent, profile(&server.base_url(), "openai", json!({}), 0), "/tmp");
    let (executor, calls) = recording_executor();
    let runtime =
        start_runtime(&core, runner, "leader", PermissionPolicy::default(), RuntimeLimits::default(), executor);
    runtime.user_message("run echo", false).unwrap();
    assert!(wait_for(|| runs(&core).iter().any(|r| !r.status.is_active()), 15_000));
    runtime.close();
    assert!(calls.lock().unwrap().is_empty(), "partial tool arguments never execute");
    assert_eq!(server.calls(), 1, "max_retries=0 allows no second attempt");
    assert_eq!(runs(&core)[0].status, TurnStatus::Failed);

    // One retry: two attempts, then the budget is exhausted.
    let partial = partial_tool.clone();
    let server = FakeOpenAi::start_sse(move |_, _| partial.clone());
    let core = core_with_spec("stream-budget-1", json!({"leader_id":"leader","agents":[agent.clone()]}));
    let runner = chat_runner(&core, &agent, profile(&server.base_url(), "openai", json!({}), 1), "/tmp");
    let (executor, calls) = recording_executor();
    let runtime =
        start_runtime(&core, runner, "leader", PermissionPolicy::default(), RuntimeLimits::default(), executor);
    runtime.user_message("run echo", false).unwrap();
    assert!(wait_for(|| runs(&core).iter().any(|r| !r.status.is_active()), 15_000));
    runtime.close();
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(server.calls(), 2, "one retry within budget, then failed");
    assert_eq!(runs(&core)[0].status, TurnStatus::Failed);

    // Visible text before the truncation: the stream is not replayed.
    let server = FakeOpenAi::start_sse(move |_, _| {
        "data: {\"choices\":[{\"delta\":{\"content\":\"half an answer\"}}]}\n\n".to_string()
    });
    let core = core_with_spec("stream-emitted", json!({"leader_id":"leader","agents":[agent]}));
    let runner = chat_runner(&core, &agent, profile(&server.base_url(), "openai", json!({}), 2), "/tmp");
    let (executor, calls) = recording_executor();
    let runtime =
        start_runtime(&core, runner, "leader", PermissionPolicy::default(), RuntimeLimits::default(), executor);
    runtime.user_message("run echo", false).unwrap();
    assert!(wait_for(|| runs(&core).iter().any(|r| !r.status.is_active()), 15_000));
    runtime.close();
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(server.calls(), 1, "a stream that already showed text is never retried");
    assert_eq!(runs(&core)[0].status, TurnStatus::Failed);
}

#[test]
fn responses_reasoning_survives_tool_continuation() {
    let _env = env_guard("responses-reasoning");
    for streaming in [false, true] {
        let reasoning = json!({"type":"reasoning","id":"rs_test","summary":[],"encrypted_content":"opaque-test-data"});
        let expected = reasoning.clone();
        let response = move |index| {
            if index == 0 {
                json!({"status":"completed","output":[reasoning,
                    {"type":"function_call","id":"fc_test","status":"completed","call_id":"call-r","name":"shell","arguments":shell_args().to_string()}]})
            } else {
                json!({"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}]})
            }
        };
        let server = if streaming {
            FakeOpenAi::start_sse(move |_, index| {
                let mut data = response(index);
                let mut wire = String::new();
                for item in data["output"].as_array().unwrap() {
                    wire.push_str(&format!("data: {}\n\n", json!({"type":"response.output_item.done","item":item})));
                }
                data["output"] = json!([]);
                wire.push_str(&format!("data: {}\n\n", json!({"type":"response.completed","response":data})));
                wire
            })
        } else {
            FakeOpenAi::start(move |_, index| (200, response(index)))
        };
        let agent = agent_json("leader", "leader", &["shell"]);
        let core = core_with_spec(
            &format!("responses-reasoning-{streaming}"),
            json!({"leader_id":"leader","agents":[agent.clone()]}),
        );
        let runner = chat_runner(&core, &agent, profile(&server.base_url(), "responses", json!({}), 0), "/tmp");
        let (executor, calls) = recording_executor();
        let runtime =
            start_runtime(&core, runner, "leader", PermissionPolicy::default(), RuntimeLimits::default(), executor);
        runtime.user_message("run echo", false).unwrap();
        assert!(wait_for(|| runs(&core).iter().any(|r| r.status == TurnStatus::Completed), 5_000));
        runtime.close();
        assert_eq!(calls.lock().unwrap().len(), 1);
        let input = server.body(1)["input"].as_array().unwrap().clone();
        let position =
            input.iter().position(|item| *item == expected).expect("the reasoning item must survive unchanged");
        assert_eq!(input[position + 1]["id"], "fc_test");
        assert_eq!(input[position + 2]["type"], "function_call_output");
        assert_eq!(input.iter().filter(|item| item["type"] == "function_call").count(), 1);
    }
}

/// `view_image` must reach the model as an actual image part: the tool result
/// stays a small reference and the bytes are read back when the request is built.
/// User-configured hooks see engine events: a tool call and the turn end.
#[test]
fn configured_hooks_see_tool_calls_and_turn_end() {
    let _env = env_guard("chat-hooks");
    let cwd = isolated_project("hooks");
    let log = cwd.join("hook.log");
    let script = cwd.join("hook.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\nprintf '%s\\n' \"$1\" >> {}\ncat >> {}\n", log.display(), log.display()),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let api = FakeOpenAi::start(|_, index| {
        (
            200,
            if index == 0 {
                tool_call_response("call-1", "write_file", json!({"path": "a.txt", "content": "x"}))
            } else {
                text_response("done")
            },
        )
    });
    let mut catalog = UserConfig::default();
    catalog.hooks.notify = vec![script.to_string_lossy().into_owned()];
    let opened = open_chat_session(&cwd, &api, &["files"], catalog);
    opened.runtime.start();
    opened.runtime.user_message("write the file", false).unwrap();
    assert!(opened.runtime.settle(10), "the turn finished");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut text = String::new();
    while std::time::Instant::now() < deadline {
        text = std::fs::read_to_string(&log).unwrap_or_default();
        if text.contains("tool_call") && text.contains("run_completed") {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(text.contains("tool_call"), "the hook saw the tool call: {text}");
    assert!(text.contains("\"tool\":\"write_file\""), "with its payload: {text}");
    assert!(text.contains("run_completed"), "and the turn end: {text}");
    opened.close();
}

/// User-configured hooks see engine events: a tool call and the turn end.
#[test]
fn view_image_attaches_the_picture_to_the_next_request() {
    let _env = env_guard("chat-view-image");
    let dir = std::env::temp_dir().join(format!(
        "ta-view-image-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    // magic bytes are what the loader validates; the payload itself is opaque
    std::fs::write(dir.join("shot.png"), [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3]).unwrap();

    let server = FakeOpenAi::start(|_body, index| match index {
        0 => (200, tool_call_response("call-1", "view_image", json!({"path": "shot.png"}))),
        _ => (200, text_response("looked at it")),
    });
    let spec = json!({"leader_id": "leader", "agents": [agent_json("leader", "leader", &["files"])]});
    let core = core_with_spec("s-view-image", spec);
    let agent = agent_json("leader", "leader", &["files"]);
    let runner =
        chat_runner(&core, &agent, profile(&server.base_url(), "openai", json!({}), 0), &dir.to_string_lossy());
    // the real workspace executor, so the tool actually reads the file
    let workspace = teamagents_engine::tools::workspace_executor(dir.clone(), None);
    let executor: ToolExecutor =
        Arc::new(move |_agent: &str, tool: &str, args: &Json, _control: &teamagents_engine::gateway::TurnControl| {
            workspace(tool, args)
        });
    let runtime =
        start_runtime(&core, runner, "leader", PermissionPolicy::default(), RuntimeLimits::default(), executor);

    runtime.user_message("look at the screenshot", false).unwrap();
    assert!(wait_for(|| server.calls() >= 2, 15_000), "the model gets a follow-up turn");
    let second = server.body(1);
    let image = second["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .flat_map(|message| message["content"].as_array().cloned().unwrap_or_default())
        .find(|part| part["type"] == "image_url")
        .unwrap_or(Json::Null);
    let url = image["image_url"]["url"].as_str().unwrap_or("").to_string();
    assert!(url.starts_with("data:image/png;base64,"), "image must travel as a data URL: {second}");
    assert!(url.len() > 30, "the bytes are really attached: {url}");
    runtime.close();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tool_activity_reaches_the_automation_sink() {
    let _env = env_guard("chat-tool-sink");
    let server = FakeOpenAi::start(|_body, index| match index {
        0 => (200, tool_call_response("call-1", "write_file", json!({"path": "note.txt", "content": "hello"}))),
        _ => (200, text_response("done")),
    });
    let spec = json!({
        "leader_id": "leader",
        "agents": [agent_json("leader", "leader", &["files"])],
    });
    let core = core_with_spec("s-tool-sink", spec);
    let agent = agent_json("leader", "leader", &["files"]);
    let notify = Notify::new(core.clone());
    let runner = chat_runner_with(&agent, profile(&server.base_url(), "openai", json!({}), 0), "/tmp", notify.clone());
    let (executor, _tool_calls) = recording_executor();
    let runtime = start_runtime_with(
        &core,
        runner,
        "leader",
        approval_policy_for_shell(),
        RuntimeLimits::default(),
        executor,
        notify.clone(),
    );

    let seen: Arc<Mutex<Vec<Json>>> = Arc::new(Mutex::new(vec![]));
    let sink = seen.clone();
    notify.set_tool_sink(Box::new(move |run_id, agent_id, activity| {
        sink.lock().unwrap().push(json!({"run_id": run_id, "agent_id": agent_id, "activity": activity.clone()}));
    }));

    runtime.user_message("write the file", false).unwrap();
    assert!(
        wait_for(|| !seen.lock().unwrap().is_empty(), 15_000),
        "tool activity must be reported while the turn runs"
    );
    let first = seen.lock().unwrap()[0].clone();
    assert_eq!(first["agent_id"], "leader");
    assert!(first["run_id"].as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(first["activity"]["tool"], "write_file");
    assert_eq!(first["activity"]["call_id"], "call-1");
    assert_eq!(first["activity"]["ok"], true);
    assert!(
        first["activity"]["arguments"].as_str().unwrap().contains("note.txt"),
        "the sink sees which file was touched: {first}"
    );
    runtime.close();
}

#[test]
fn once_approval_is_consumed_and_the_turn_completes() {
    let _env = env_guard("chat-once");
    let server = FakeOpenAi::start(|_body, index| match index {
        0 => (200, tool_call_response("call-1", "shell", shell_args())),
        1 => (200, tool_call_response("call-2", "shell", shell_args())),
        _ => (200, text_response("all done")),
    });
    let spec = json!({
        "leader_id": "leader",
        "agents": [agent_json("leader", "leader", &["shell"])],
    });
    let core = core_with_spec("s-once", spec.clone());
    let agent = agent_json("leader", "leader", &["shell"]);
    let runner = chat_runner(&core, &agent, profile(&server.base_url(), "openai", json!({}), 0), "/tmp");
    let (executor, tool_calls) = recording_executor();
    let runtime =
        start_runtime(&core, runner, "leader", approval_policy_for_shell(), RuntimeLimits::default(), executor);

    runtime.user_message("run the shell", false).unwrap();
    assert!(
        // CI 的慢机器上给足预算：这里只影响失败时报错的延迟，不影响判定
        wait_for(|| !pending_approvals(&core).is_empty(), 20_000),
        "the shell call parks for approval (events {:?})",
        event_kinds(&core)
    );
    let first = pending_approvals(&core)[0]["approval_id"].as_str().unwrap().to_string();

    submit(&core, "dec-1", "user", "approval_decision", json!({"approval_id": first, "decision": "once"}));
    assert!(
        wait_for(
            || {
                let rows = runs(&core);
                rows.iter().any(|r| r.status == TurnStatus::Completed)
                    && !rows.iter().any(|r| r.status.is_active())
                    && pending_approvals(&core).is_empty()
            },
            30_000
        ),
        "the turn completes after the once approval (runs {:?}, pending {}, events {:?})",
        runs(&core).iter().map(|r| serde_json::to_value(r.status).unwrap_or(Json::Null)).collect::<Vec<_>>(),
        pending_approvals(&core).len(),
        event_kinds(&core)
    );

    let called = tool_calls.lock().unwrap().clone();
    assert_eq!(called.len(), 1, "the approved operation ran exactly once: {called:?}");
    assert_eq!(called[0].0, "shell");
    assert_eq!(called[0].1, shell_args());
    assert_eq!(approval_status(&core, &first), "EXPIRED", "a once approval is single use");
    assert!(event_kinds(&core).contains(&"approval_decided".to_string()));
    runtime.close();
}

/// F-12: an already-consumed (EXPIRED) approval must not authorize a repeat of
/// the same operation — a new request is raised and the user decides it.
#[test]
fn expired_once_approval_requires_a_new_request() {
    let _env = env_guard("chat-expired");
    let server = FakeOpenAi::start(|_body, index| match index {
        0 => (200, tool_call_response("call-1", "shell", shell_args())),
        1 => (200, tool_call_response("call-2", "shell", shell_args())),
        2 => (200, tool_call_response("call-3", "shell", shell_args())),
        _ => (200, text_response("done")),
    });
    let spec = json!({
        "leader_id": "leader",
        "agents": [agent_json("leader", "leader", &["shell"])],
    });
    let core = core_with_spec("s-expired", spec);
    let agent = agent_json("leader", "leader", &["shell"]);
    let runner = chat_runner(&core, &agent, profile(&server.base_url(), "openai", json!({}), 0), "/tmp");
    let (executor, tool_calls) = recording_executor();
    let runtime =
        start_runtime(&core, runner, "leader", approval_policy_for_shell(), RuntimeLimits::default(), executor);

    runtime.user_message("run the shell", false).unwrap();
    assert!(wait_for(|| !pending_approvals(&core).is_empty(), 10_000));
    let first = pending_approvals(&core)[0]["approval_id"].as_str().unwrap().to_string();
    submit(&core, "dec-1", "user", "approval_decision", json!({"approval_id": first, "decision": "once"}));

    // the model repeats the identical operation: the consumed approval must not
    // cover it, so a second request appears while the operation ran once
    assert!(
        wait_for(
            || {
                let pending = pending_approvals(&core);
                !pending.is_empty() && tool_calls.lock().unwrap().len() == 1
            },
            15_000
        ),
        "the repeated operation parks again (executed once so far: {:?})",
        tool_calls.lock().unwrap().len()
    );
    let second = pending_approvals(&core)[0]["approval_id"].as_str().unwrap().to_string();
    assert_ne!(first, second, "a consumed approval cannot be reused");
    assert_eq!(approval_status(&core, &first), "EXPIRED");

    submit(&core, "dec-2", "user", "approval_decision", json!({"approval_id": second, "decision": "deny"}));
    assert!(
        wait_for(
            || {
                let rows = runs(&core);
                !rows.iter().any(|r| r.status.is_active()) && pending_approvals(&core).is_empty()
            },
            15_000
        ),
        "the turn finishes after the denial"
    );
    assert_eq!(tool_calls.lock().unwrap().len(), 1, "the denied repeat never ran");
    runtime.close();
}

/// A denied operation never reaches the executor and the member moves on.
#[test]
fn denied_approval_blocks_the_operation() {
    let _env = env_guard("chat-deny");
    let server = FakeOpenAi::start(|_body, index| match index {
        0 => (200, tool_call_response("call-1", "shell", shell_args())),
        // the member re-sends the same operation after the decision; the
        // recorded denial must answer it instead of parking a new request
        1 => (200, tool_call_response("call-2", "shell", shell_args())),
        _ => (200, text_response("understood")),
    });
    let core = core_with_spec(
        "s-deny",
        json!({"leader_id": "leader", "agents": [agent_json("leader", "leader", &["shell"])]}),
    );
    let agent = agent_json("leader", "leader", &["shell"]);
    let runner = chat_runner(&core, &agent, profile(&server.base_url(), "openai", json!({}), 0), "/tmp");
    let (executor, tool_calls) = recording_executor();
    let runtime =
        start_runtime(&core, runner, "leader", approval_policy_for_shell(), RuntimeLimits::default(), executor);

    runtime.user_message("run the shell", false).unwrap();
    assert!(wait_for(|| !pending_approvals(&core).is_empty(), 10_000));
    let aid = pending_approvals(&core)[0]["approval_id"].as_str().unwrap().to_string();
    submit(&core, "dec-1", "user", "approval_decision", json!({"approval_id": aid, "decision": "deny"}));

    assert!(
        wait_for(
            || {
                let rows = runs(&core);
                rows.iter().any(|r| r.status == TurnStatus::Completed)
                    && !rows.iter().any(|r| r.status.is_active())
                    && pending_approvals(&core).is_empty()
            },
            15_000
        ),
        "the turn completes after the denial"
    );
    assert!(tool_calls.lock().unwrap().is_empty(), "the denied operation never ran");
    assert_eq!(approval_status(&core, &aid), "DENIED");
    assert_eq!(server.calls(), 3, "the member re-sent the call and got a final answer");
    runtime.close();
}

#[test]
fn private_subagent_uses_parent_bindings_without_team_identity_or_history() {
    let _env = env_guard("chat-private-subagent");
    let server = FakeOpenAi::start(|body, _index| {
        if private_helper_request(body) {
            if body_has_role(body, "tool") {
                (200, text_response("helper inspected the workspace"))
            } else {
                (200, tool_call_response("child-write", "write_file", json!({"path": "child.txt", "content": "ok"})))
            }
        } else if body_has_role(body, "tool") {
            (200, text_response("parent received the private result"))
        } else {
            (
                200,
                tool_call_response(
                    "parent-subagent",
                    "run_subagent",
                    json!({"task": "Inspect the workspace and leave a small evidence file.", "context": "helper-only-context"}),
                ),
            )
        }
    });
    let agent = agent_json("leader", "leader", &["files"]);
    let core = core_with_spec("s-private-subagent", json!({"leader_id":"leader", "agents":[agent.clone()]}));
    let notify = Notify::new(core.clone());
    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let event_sink = events.clone();
    notify.set_event_sink(Box::new(move |kind, _payload| event_sink.lock().unwrap().push(kind.to_string())));
    let runner = chat_runner_with(&agent, profile(&server.base_url(), "openai", json!({}), 0), "/tmp", notify.clone());
    let (executor, calls) = recording_executor();
    let runtime = start_runtime_with(
        &core,
        runner,
        "leader",
        PermissionPolicy::default(),
        RuntimeLimits::default(),
        executor,
        notify,
    );

    runtime.user_message("parent-secret: keep this out of the helper", false).unwrap();
    assert!(
        wait_for(|| !runs(&core).iter().any(|run| run.status.is_active()), 15_000),
        "the parent and private helper finish"
    );

    assert_eq!(calls.lock().unwrap().as_slice(), [("write_file".into(), json!({"path":"child.txt", "content":"ok"}))]);
    assert!(server.calls() >= 4, "parent, helper tool, helper final, parent final: {}", server.calls());
    let helper = server.body(1);
    assert!(private_helper_request(&helper), "the nested request has its private environment prompt: {helper}");
    assert!(helper.to_string().contains("helper-only-context"));
    assert!(!helper.to_string().contains("parent-secret"), "parent history leaked into helper: {helper}");
    let helper_tools: Vec<&str> = helper["tools"]
        .as_array()
        .map(|tools| tools.as_slice())
        .unwrap_or(&[])
        .iter()
        .filter_map(|tool| tool.pointer("/function/name").and_then(|name| name.as_str()))
        .collect();
    assert!(helper_tools.contains(&"write_file"));
    assert!(!helper_tools.contains(&"send_message"));
    assert!(!helper_tools.contains(&"run_subagent"));
    assert!(!helper_tools.contains(&"update_plan"));
    let parent_followup = server.body(server.calls() - 1);
    assert!(parent_followup.to_string().contains("helper inspected the workspace"));
    assert!(events.lock().unwrap().contains(&"private_subagent_tool_call".to_string()));
    runtime.close();
}

#[test]
fn private_subagent_approval_resumes_nested_tool_once_and_keeps_protocol_order() {
    let _env = env_guard("chat-private-approval");
    let server = FakeOpenAi::start(|body, _index| {
        if private_helper_request(body) {
            if body_has_role(body, "tool") {
                (200, text_response("network check completed"))
            } else {
                (
                    200,
                    tool_call_response(
                        "child-network",
                        "shell",
                        json!({"command": "echo private-network", "network": true}),
                    ),
                )
            }
        } else if body_has_role(body, "tool") {
            (200, text_response("parent continued after approval"))
        } else {
            (
                200,
                tool_call_response(
                    "parent-approval",
                    "run_subagent",
                    json!({"task": "Run the approved network probe.", "context": "approval regression"}),
                ),
            )
        }
    });
    let agent = agent_json("leader", "leader", &["shell"]);
    let core = core_with_spec("s-private-approval", json!({"leader_id":"leader", "agents":[agent.clone()]}));
    let notify = Notify::new(core.clone());
    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let event_sink = events.clone();
    notify.set_event_sink(Box::new(move |kind, _payload| event_sink.lock().unwrap().push(kind.to_string())));
    let runner = chat_runner_with(&agent, profile(&server.base_url(), "openai", json!({}), 0), "/tmp", notify.clone());
    let (executor, calls) = recording_executor();
    let runtime = start_runtime_with(
        &core,
        runner,
        "leader",
        approval_policy_for_shell(),
        RuntimeLimits::default(),
        executor,
        notify,
    );

    runtime.user_message("ask the helper to use the network", false).unwrap();
    assert!(wait_for(|| !pending_approvals(&core).is_empty(), 15_000), "the helper shell call needs approval");
    let approval_id = pending_approvals(&core)[0]["approval_id"].as_str().unwrap().to_string();
    submit(
        &core,
        "private-approval-once",
        "user",
        "approval_decision",
        json!({"approval_id": approval_id, "decision": "once"}),
    );
    assert!(
        wait_for(
            || runs(&core).iter().any(|run| run.status == TurnStatus::Completed)
                && !runs(&core).iter().any(|run| run.status.is_active()),
            20_000,
        ),
        "the nested helper resumes after approval"
    );

    assert_eq!(calls.lock().unwrap().len(), 1, "the approved child shell executes exactly once");
    assert_eq!(calls.lock().unwrap()[0].0, "shell");
    assert_eq!(approval_status(&core, &approval_id), "EXPIRED");
    let resumed_helper = (0..server.calls())
        .map(|index| server.body(index))
        .find(|body| private_helper_request(body) && body_has_role(body, "tool"))
        .expect("a resumed helper request contains the child tool result");
    let messages = resumed_helper["messages"].as_array().unwrap();
    let assistant = messages.iter().position(|message| message["role"] == "assistant").unwrap();
    let tool = messages.iter().position(|message| message["role"] == "tool").unwrap();
    assert_eq!(tool, assistant + 1, "the resume keeps assistant tool call/result ordering: {resumed_helper}");
    assert!(events.lock().unwrap().contains(&"private_subagent_tool_call".to_string()));
    runtime.close();
}

#[test]
fn private_subagent_model_failure_is_returned_to_parent_and_parent_can_continue() {
    let _env = env_guard("chat-private-failure");
    let server = FakeOpenAi::start(|body, _index| {
        if private_helper_request(body) {
            (500, json!({"error":{"message":"helper backend unavailable"}}))
        } else if body_has_role(body, "tool") {
            assert!(body.to_string().contains("private helper model error"));
            (200, text_response("parent handled helper failure"))
        } else {
            (
                200,
                tool_call_response(
                    "parent-failure",
                    "run_subagent",
                    json!({"task": "Try the unavailable helper.", "context": "failure regression"}),
                ),
            )
        }
    });
    let agent = agent_json("leader", "leader", &[]);
    let core = core_with_spec("s-private-failure", json!({"leader_id":"leader", "agents":[agent.clone()]}));
    let runner = chat_runner(&core, &agent, profile(&server.base_url(), "openai", json!({}), 0), "/tmp");
    let (executor, _calls) = recording_executor();
    let runtime =
        start_runtime(&core, runner, "leader", PermissionPolicy::default(), RuntimeLimits::default(), executor);
    runtime.user_message("continue even if the helper fails", false).unwrap();
    assert!(wait_for(|| !runs(&core).iter().any(|run| run.status.is_active()), 15_000));
    assert_eq!(runs(&core)[0].status, TurnStatus::Completed);
    runtime.close();
}

#[test]
fn private_subagent_model_steps_share_the_parent_turn_budget() {
    let _env = env_guard("chat-private-budget");
    let server = FakeOpenAi::start(|body, _index| {
        if private_helper_request(body) {
            (200, text_response("helper would need another step"))
        } else {
            (
                200,
                tool_call_response(
                    "parent-budget",
                    "run_subagent",
                    json!({"task": "Use one helper model step.", "context": "budget regression"}),
                ),
            )
        }
    });
    let agent = agent_json("leader", "leader", &[]);
    let core = core_with_spec(
        "s-private-budget",
        json!({"leader_id":"leader", "agents":[agent.clone()], "limits":{"max_model_steps_per_turn":1}}),
    );
    let runner = chat_runner(&core, &agent, profile(&server.base_url(), "openai", json!({}), 0), "/tmp");
    let (executor, _calls) = recording_executor();
    let runtime =
        start_runtime(&core, runner, "leader", PermissionPolicy::default(), RuntimeLimits::default(), executor);
    runtime.user_message("the parent call itself consumes the only step", false).unwrap();
    assert!(wait_for(|| !runs(&core).iter().any(|run| run.status.is_active()), 15_000));
    assert_eq!(runs(&core)[0].status, TurnStatus::Failed);
    assert_eq!(server.calls(), 1, "the helper cannot start after the parent exhausted the budget");
    assert!(event_kinds(&core).contains(&"limit_reached".to_string()));
    runtime.close();
}

#[test]
fn private_subagent_receipt_recovers_without_replaying_the_child_tool() {
    let _env = env_guard("chat-private-receipt-recovery");
    let server = FakeOpenAi::start(|body, _index| {
        if private_helper_request(body) {
            assert!(body_has_role(body, "tool"), "the recovered child result precedes the helper resume: {body}");
            (200, text_response("helper resumed from the durable child receipt"))
        } else {
            assert!(body_has_role(body, "tool"), "the parent receives the helper result: {body}");
            (200, text_response("parent resumed without repeating the child operation"))
        }
    });
    let agent = agent_json("leader", "leader", &["shell"]);
    let core = core_with_spec("s-private-receipt", json!({"leader_id":"leader", "agents":[agent.clone()]}));
    let runner = chat_runner(&core, &agent, profile(&server.base_url(), "openai", json!({}), 0), "/tmp");
    let run: TurnRun = serde_json::from_value(json!({
        "run_id":"receipt-recovery", "session_id":"s-private-receipt", "task_id":null, "goal_id":null,
        "agent_id":"leader", "config_revision":1, "topology_revision":1, "status":"QUEUED",
        "input_delivery_ids":[], "context_ref":"ctx:receipt-recovery", "external_turn_id":null,
        "cancel_requested":false, "waiting_on":[], "created_at":0, "updated_at":0
    }))
    .unwrap();

    // Model the durable state immediately after the child operation returned
    // and its receipt was checkpointed, but before the nested tool message was
    // appended. The external operation is represented by this receipt; the
    // executor must not be called again during recovery.
    let checkpoint = json!({
        "history":[
            {"role":"system", "content":"old parent system"},
            {"role":"user", "content":"resume the private helper"},
            {"role":"assistant", "content":null, "tool_calls":[{
                "id":"parent-receipt", "type":"function",
                "function":{"name":"run_subagent", "arguments":
                    "{\"task\":\"resume the helper\",\"context\":\"receipt regression\"}"}
            }]}
        ],
        "model_steps":1,
        "pending_external":null,
        "outcome":null,
        "input_events":[],
        "delivery_ids":[],
        "tree_base":0,
        "tree_leaf":null,
        "rewind_epoch":0,
        "tree_pending":[],
        "private_subagent":{
            "parent_call_id":"parent-receipt",
            "task":"resume the helper",
            "context":"receipt regression",
            "history":[
                {"role":"system", "content":"<teamagents_private_subagent>private helper</teamagents_private_subagent>"},
                {"role":"user", "content":"<private_task>resume the helper</private_task>"},
                {"role":"assistant", "content":null, "tool_calls":[{
                    "id":"child-receipt", "type":"function",
                    "function":{"name":"shell", "arguments":"{\"command\":\"echo once\"}"}
                }]}
            ],
            "result":null,
            "pending_tool":{"call_id":"child-receipt", "name":"shell", "args":{"command":"echo once"}},
            "tool_receipt":{"ok":true, "result":{"output":"already executed"}, "error":null},
            "tool_attempted":true
        }
    });
    let checkpoint_path = runner.history_dir().unwrap().join("turns/receipt-recovery.json");
    std::fs::create_dir_all(checkpoint_path.parent().unwrap()).unwrap();
    std::fs::write(&checkpoint_path, checkpoint.to_string()).unwrap();

    let executed: RecordedCalls = Arc::new(Mutex::new(vec![]));
    let sink = executed.clone();
    let executor = Arc::new(move |tool: &str, args: &Json| {
        sink.lock().unwrap().push((tool.to_string(), args.clone()));
        Ok(json!({"output":"unexpected replay"}))
    });
    let gateway = ToolGateway::new(
        core.clone(),
        "leader",
        "receipt-recovery",
        ApprovalGate::new(core, PermissionPolicy::default()),
        Some(executor),
    );
    let view = json!({"inbox_delta":[], "delivery_ids":[], "relevant_topology":{"revision":1}});
    let outcome = runner.start_or_resume(&run, &view, &gateway, &Json::Null);

    assert_eq!(outcome.status, TurnStatus::Completed, "{outcome:?}");
    assert_eq!(outcome.reply_text.as_deref(), Some("parent resumed without repeating the child operation"));
    assert_eq!(server.calls(), 2, "helper resume and parent continuation only");
    assert!(executed.lock().unwrap().is_empty(), "the child operation was replayed");
    runner.close();
}

/// F-1: the spec's max_model_steps_per_turn caps *model requests*; hitting it
/// fails the turn and the core emits `limit_reached`.
#[test]
fn model_step_limit_reports_limit_reached() {
    let _env = env_guard("chat-step");
    let server = FakeOpenAi::start(|_body, _index| (200, tool_call_response("call-x", "list_shared", json!({}))));
    let core = core_with_spec(
        "s-step",
        json!({"leader_id": "leader", "agents": [agent_json("leader", "leader", &[])],
               "limits": {"max_model_steps_per_turn": 3}}),
    );
    let agent = agent_json("leader", "leader", &[]);
    let runner = chat_runner(&core, &agent, profile(&server.base_url(), "openai", json!({}), 0), "/tmp");
    let (executor, _calls) = recording_executor();
    let runtime =
        start_runtime(&core, runner, "leader", PermissionPolicy::default(), RuntimeLimits::default(), executor);

    runtime.user_message("spin forever", false).unwrap();
    assert!(
        wait_for(|| runs(&core).iter().all(|r| r.status.is_terminal()), 15_000),
        "the turn stops at the model-step limit"
    );
    let state = core.state().unwrap();
    let run = &runs(&core)[0];
    assert_eq!(run.status, TurnStatus::Failed);
    assert_eq!(server.calls(), 3, "no fourth model request is made");
    let kinds = event_kinds(&core);
    assert!(kinds.contains(&"limit_reached".to_string()), "limit_reached missing from {kinds:?}");
    let error = state
        .get("events")
        .and_then(|v| v.as_array())
        .and_then(|events| events.iter().rev().find(|e| e.get("kind").and_then(|v| v.as_str()) == Some("run_failed")))
        .and_then(|e| e.pointer("/payload/error").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_default();
    assert!(error.contains("model-step limit 3"), "unexpected error {error:?}");
    runtime.close();
}

/// F-2/F-9: after the active-time limit the member is interrupted; no further
/// model calls or shared-space writes land.
#[test]
fn timeout_interrupts_the_member_before_further_side_effects() {
    let _env = env_guard("chat-timeout");
    // slow enough that an uninterruptible loop could not finish inside the
    // observation window (the model-step budget alone would take minutes)
    let server = FakeOpenAi::start(|_body, index| {
        std::thread::sleep(std::time::Duration::from_millis(30));
        (
            200,
            tool_call_response(
                &format!("call-{index}"),
                "publish_shared",
                json!({"space_id": "main", "content": format!("tick-{index}")}),
            ),
        )
    });
    let core = core_with_spec(
        "s-timeout",
        json!({
            "leader_id": "leader",
            "agents": [agent_json("leader", "leader", &[])],
            "shared_spaces": [{"id": "main", "readers": ["leader"], "writers": ["leader"]}],
            "limits": {"turn_active_timeout_s": 2},
        }),
    );
    let agent = agent_json("leader", "leader", &[]);
    let runner = chat_runner(&core, &agent, profile(&server.base_url(), "openai", json!({}), 0), "/tmp");
    let (executor, _calls) = recording_executor();
    let runtime =
        start_runtime(&core, runner, "leader", PermissionPolicy::default(), RuntimeLimits::default(), executor);

    runtime.user_message("publish forever", false).unwrap();
    assert!(
        wait_for(|| !runs(&core).is_empty() && runs(&core).iter().all(|r| r.status.is_terminal()), 20_000),
        "the turn is finalised on timeout"
    );
    let entry_count = |core: &Arc<CoreClient>| -> usize {
        core.call_in_session("shared_entries", json!({"space_ids": ["main"]}))
            .ok()
            .and_then(|reply| reply.get("entries").and_then(|v| v.as_array()).map(|a| a.len()))
            .unwrap_or(0)
    };
    assert!(entry_count(&core) > 0, "side effects happened inside the turn (control)");
    let (entries, calls) = (entry_count(&core), server.calls());
    std::thread::sleep(std::time::Duration::from_millis(1500));
    // at most the call already in flight when the timeout fired
    assert!(
        server.calls() - calls <= 1,
        "the member keeps calling the model after the timeout: {calls} -> {}",
        server.calls()
    );
    assert!(
        entry_count(&core) - entries <= 1,
        "side effects keep landing after the timeout: {entries} -> {}",
        entry_count(&core)
    );
    let status = runs(&core)[0].status;
    assert_eq!(status, TurnStatus::Failed);
    runtime.close();
}

/// F-9: a member thread that crashes is reported as a crash, not as a timeout.
#[test]
fn a_crashed_member_is_not_reported_as_a_timeout() {
    let _env = env_guard("chat-panic");
    let core =
        core_with_spec("s-panic", json!({"leader_id": "leader", "agents": [agent_json("leader", "leader", &[])]}));
    let notify = Notify::new(core.clone());
    let approvals = ApprovalGate::new(core.clone(), PermissionPolicy::default());
    let executor: ToolExecutor =
        Arc::new(|_agent: &str, tool: &str, _args: &Json, _control: &teamagents_engine::gateway::TurnControl| {
            Err(format!("no executor for {tool}"))
        });
    let runtime = Runtime::new(core.clone(), notify, approvals, executor, None, RuntimeLimits::default());
    runtime.add_runner("leader", Arc::new(PanicRunner));
    runtime.start();

    runtime.user_message("trigger the crash", false).unwrap();
    assert!(
        wait_for(
            || {
                let rows = runs(&core);
                !rows.is_empty() && !rows.iter().any(|r| r.status.is_active())
            },
            10_000
        ),
        "the crashed turn is finalised"
    );
    let error = core
        .state()
        .unwrap()
        .get("events")
        .and_then(|v| v.as_array())
        .and_then(|events| events.iter().rev().find(|e| e.get("kind").and_then(|v| v.as_str()) == Some("run_failed")))
        .and_then(|e| e.pointer("/payload/error").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_default();
    assert!(error.contains("member runner crashed"), "unexpected error {error:?}");
    assert!(!error.contains("active-time limit"), "a crash must not be reported as a timeout: {error:?}");
    runtime.close();
}

/// F-4: the member's conversation history is reloaded after a restart.
#[test]
fn conversation_history_survives_a_restart() {
    let _env = env_guard("chat-history");
    let server = FakeOpenAi::start(|_body, _index| (200, text_response("first-reply")));
    let core =
        core_with_spec("s-history", json!({"leader_id": "leader", "agents": [agent_json("leader", "leader", &[])]}));
    let agent = agent_json("leader", "leader", &[]);
    let run = |run_id: &str| -> TurnRun {
        serde_json::from_value(json!({
            "run_id": run_id, "session_id": "s-history", "task_id": null, "goal_id": null,
            "agent_id": "leader", "config_revision": 1, "topology_revision": 1, "status": "QUEUED",
            "input_delivery_ids": [], "context_ref": "ctx:1", "external_turn_id": null,
            "cancel_requested": false, "waiting_on": [], "created_at": 0, "updated_at": 0,
        }))
        .unwrap()
    };
    let view = json!({"agent_id": "leader", "assignment": [], "inbox_delta": [], "permitted_shared_delta": [],
                      "relevant_topology": {"revision": 1}, "delivery_ids": [], "batch_no": 0});
    let gateway = ToolGateway::new(
        core.clone(),
        "leader",
        "run-1",
        ApprovalGate::new(core.clone(), PermissionPolicy::default()),
        Some(Arc::new(|tool: &str, _args: &Json| Err(format!("no executor for {tool}")))),
    );

    let first = chat_runner(&core, &agent, profile(&server.base_url(), "openai", json!({}), 0), "/tmp");
    let outcome = first.start_or_resume(&run("run-1"), &view, &gateway, &json!({"reason": "new_input"}));
    assert_eq!(outcome.status, TurnStatus::Completed);
    drop(first);

    // simulated restart: a fresh runner for the same member and session
    let second = chat_runner(&core, &agent, profile(&server.base_url(), "openai", json!({}), 0), "/tmp");
    let outcome = second.start_or_resume(&run("run-2"), &view, &gateway, &json!({"reason": "new_input"}));
    assert_eq!(outcome.status, TurnStatus::Completed);
    let messages = server.body(1).get("messages").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    assert!(
        messages.iter().any(|m| m.get("content").and_then(|v| v.as_str()) == Some("first-reply")),
        "the previous turn's reply is in the restarted member's context: {}",
        serde_json::to_string(&messages).unwrap_or_default()
    );
}

/// F-11: 4xx client errors are not retried; 5xx are retried up to max_retries.
#[test]
fn retry_policy_only_retries_transient_errors() {
    let _env = env_guard("chat-retry");
    let unauthorized = FakeOpenAi::start(|_body, _index| (401, json!({"error": {"message": "bad key"}})));
    let core =
        core_with_spec("s-retry-401", json!({"leader_id": "leader", "agents": [agent_json("leader", "leader", &[])]}));
    let agent = agent_json("leader", "leader", &[]);
    let run: TurnRun = serde_json::from_value(json!({
        "run_id": "run-401", "session_id": "s-retry-401", "task_id": null, "goal_id": null,
        "agent_id": "leader", "config_revision": 1, "topology_revision": 1, "status": "QUEUED",
        "input_delivery_ids": [], "context_ref": "ctx:1", "external_turn_id": null,
        "cancel_requested": false, "waiting_on": [], "created_at": 0, "updated_at": 0,
    }))
    .unwrap();
    let view = json!({"agent_id": "leader", "assignment": [], "inbox_delta": [], "permitted_shared_delta": [],
                      "relevant_topology": {"revision": 1}, "delivery_ids": [], "batch_no": 0});
    let gateway = ToolGateway::new(
        core.clone(),
        "leader",
        "run-401",
        ApprovalGate::new(core.clone(), PermissionPolicy::default()),
        None,
    );
    let runner = chat_runner(&core, &agent, profile(&unauthorized.base_url(), "openai", json!({}), 2), "/tmp");
    let outcome = runner.start_or_resume(&run, &view, &gateway, &json!({"reason": "new_input"}));
    assert_eq!(outcome.status, TurnStatus::Failed);
    assert!(outcome.error.unwrap_or_default().contains("401"));
    assert_eq!(unauthorized.calls(), 1, "a 401 is not retried");

    let broken = FakeOpenAi::start(|_body, _index| (500, json!({"error": {"message": "boom"}})));
    let core =
        core_with_spec("s-retry-500", json!({"leader_id": "leader", "agents": [agent_json("leader", "leader", &[])]}));
    let gateway = ToolGateway::new(
        core.clone(),
        "leader",
        "run-500",
        ApprovalGate::new(core.clone(), PermissionPolicy::default()),
        None,
    );
    let run500: TurnRun = serde_json::from_value(json!({
        "run_id": "run-500", "session_id": "s-retry-500", "task_id": null, "goal_id": null,
        "agent_id": "leader", "config_revision": 1, "topology_revision": 1, "status": "QUEUED",
        "input_delivery_ids": [], "context_ref": "ctx:1", "external_turn_id": null,
        "cancel_requested": false, "waiting_on": [], "created_at": 0, "updated_at": 0,
    }))
    .unwrap();
    let runner = chat_runner(&core, &agent, profile(&broken.base_url(), "openai", json!({}), 2), "/tmp");
    let outcome = runner.start_or_resume(&run500, &view, &gateway, &json!({"reason": "new_input"}));
    assert_eq!(outcome.status, TurnStatus::Failed);
    assert!(outcome.error.unwrap_or_default().contains("500"));
    assert_eq!(broken.calls(), 3, "one attempt plus max_retries=2");
}

/// F-7: deepseek normalizes xhigh to max; other protocols retry once with max
/// after the provider rejects the requested effort.
#[test]
fn effort_normalization_and_fallback() {
    let _env = env_guard("chat-effort");
    // deepseek: the request never carries xhigh
    let seen = Arc::new(Mutex::new(vec![]));
    let sink = seen.clone();
    let deepseek = FakeOpenAi::start(move |body, _index| {
        sink.lock().unwrap().push(body.get("reasoning_effort").cloned().unwrap_or(Json::Null));
        (200, text_response("ok"))
    });
    let core =
        core_with_spec("s-effort-ds", json!({"leader_id": "leader", "agents": [agent_json("leader", "leader", &[])]}));
    let agent = agent_json("leader", "leader", &[]);
    let view = json!({"agent_id": "leader", "assignment": [], "inbox_delta": [], "permitted_shared_delta": [],
                      "relevant_topology": {"revision": 1}, "delivery_ids": [], "batch_no": 0});
    let run = |run_id: &str, session: &str| -> TurnRun {
        serde_json::from_value(json!({
            "run_id": run_id, "session_id": session, "task_id": null, "goal_id": null,
            "agent_id": "leader", "config_revision": 1, "topology_revision": 1, "status": "QUEUED",
            "input_delivery_ids": [], "context_ref": "ctx:1", "external_turn_id": null,
            "cancel_requested": false, "waiting_on": [], "created_at": 0, "updated_at": 0,
        }))
        .unwrap()
    };
    let gateway = ToolGateway::new(
        core.clone(),
        "leader",
        "run-effort-ds",
        ApprovalGate::new(core.clone(), PermissionPolicy::default()),
        None,
    );
    let runner = chat_runner(
        &core,
        &agent,
        profile(&deepseek.base_url(), "deepseek", json!({"reasoning_effort": "xhigh"}), 0),
        "/tmp",
    );
    let outcome =
        runner.start_or_resume(&run("run-effort-ds", "s-effort-ds"), &view, &gateway, &json!({"reason": "new_input"}));
    assert_eq!(outcome.status, TurnStatus::Completed);
    assert_eq!(seen.lock().unwrap()[0], json!("max"), "deepseek never sees xhigh");

    // openai: rejected xhigh retries once with max
    let rejected = FakeOpenAi::start(|body, _index| {
        if body.get("reasoning_effort").and_then(|v| v.as_str()) == Some("xhigh") {
            (400, json!({"error": {"message": "unsupported value: reasoning_effort xhigh"}}))
        } else {
            (200, text_response("ok"))
        }
    });
    let core =
        core_with_spec("s-effort-oa", json!({"leader_id": "leader", "agents": [agent_json("leader", "leader", &[])]}));
    let gateway = ToolGateway::new(
        core.clone(),
        "leader",
        "run-effort-oa",
        ApprovalGate::new(core.clone(), PermissionPolicy::default()),
        None,
    );
    let runner = chat_runner(
        &core,
        &agent,
        profile(&rejected.base_url(), "openai", json!({"reasoning_effort": "xhigh"}), 0),
        "/tmp",
    );
    let outcome =
        runner.start_or_resume(&run("run-effort-oa", "s-effort-oa"), &view, &gateway, &json!({"reason": "new_input"}));
    assert_eq!(outcome.status, TurnStatus::Completed);
    assert_eq!(rejected.calls(), 2, "one rejection plus one fallback retry");
    assert_eq!(rejected.body(0)["reasoning_effort"], json!("xhigh"));
    assert_eq!(rejected.body(1)["reasoning_effort"], json!("max"));
}

// Follow-up review regressions: real member, process and sandbox boundaries.
use std::time::Duration;
use teamagents_engine::session::{open_session, OpenOptions};

fn isolated_project(tag: &str) -> std::path::PathBuf {
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("ta-review-{tag}-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // env_guard owns XDG config/state outside the member's project root.
    dir
}

fn open_chat_session(
    cwd: &std::path::Path,
    api: &FakeOpenAi,
    bindings: &[&str],
    mut catalog: UserConfig,
) -> Arc<teamagents_engine::session::OpenedSession> {
    catalog.models.insert("m".into(), profile(&api.base_url(), "openai", json!({}), 0));
    catalog.models.insert(
        "other".into(),
        ModelProfile { model: "new-model".into(), ..profile(&api.base_url(), "openai", json!({}), 0) },
    );
    open_session(OpenOptions {
        cwd: Some(cwd.to_path_buf()),
        session_id: Some("review".into()),
        initial_spec: Some(json!({"leader_id":"leader", "agents":[agent_json("leader", "leader", bindings)]})),
        catalog: Some(catalog),
        ..Default::default()
    })
    .unwrap()
}

#[test]
fn persisted_work_is_checked_before_session_start_and_resumes_after_repair() {
    use teamagents_core::control::Control;
    use teamagents_core::models::{ActionKind, ApprovalRequest, Task, TeamAction, TeamSpec};
    use teamagents_core::storage::Store;
    use teamagents_engine::sessions::{is_session_locked, session_paths};

    // Snapshot raw values, including invalid JSON and original permission mode.
    fn snapshot(store: &Store) -> Json {
        let mut state = serde_json::Map::new();
        for table in [
            "sessions",
            "team_specs",
            "tasks",
            "turn_runs",
            "approvals",
            "events",
            "deliveries",
            "agent_runtime",
            "actions",
        ] {
            let mut statement = store.conn.prepare(&format!("SELECT * FROM {table} ORDER BY rowid")).unwrap();
            let columns = statement.column_count();
            let rows: Vec<Vec<String>> = statement
                .query_map([], |row| {
                    (0..columns).map(|index| row.get_ref(index).map(|value| format!("{value:?}"))).collect()
                })
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            state.insert(table.into(), json!(rows));
        }
        json!(state)
    }

    let env = env_guard("stored-work-integrity");
    let api = FakeOpenAi::start(|_, index| {
        (
            200,
            match index % 3 {
                0 => tool_call_response(
                    "report",
                    "write_file",
                    json!({"path":"report.txt","content":"recovered once\n"}),
                ),
                1 => tool_call_response("done", "signal_done", json!({"summary":"recovered"})),
                _ => text_response("Recovered the original input."),
            },
        )
    });
    let mut catalog = UserConfig::default();
    catalog.models.insert("m".into(), profile(&api.base_url(), "openai", json!({}), 0));
    let spec = json!({"leader_id":"leader","agents":[agent_json("leader","leader",&["files"])]});
    for (index, (table, field, bad)) in [
        ("tasks", "dependencies", "["),
        ("tasks", "result_refs", "[true]"),
        ("turn_runs", "input_delivery_ids", r#"["PRIVATE_BAD_VALUE"]"#),
        ("turn_runs", "waiting_on", "{}"),
        ("approvals", "requested_scope", "{"),
        ("events", "payload_json", "{"),
        ("events", "audience_json", "[false]"),
        ("events", "kind", "future_event"),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("stored-work-{index}");
        let cwd = env.join(format!("project-{index}"));
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(cwd.join("sentinel.txt"), "original user file\n").unwrap();
        let paths = session_paths(&id);
        std::fs::create_dir_all(&paths.base).unwrap();
        let store = Store::open(&paths.db).unwrap();
        store.create_session(&id, cwd.to_str().unwrap(), "approved_scope").unwrap();
        let team: TeamSpec = serde_json::from_value(spec.clone()).unwrap();
        store.save_team_spec(&id, &team).unwrap();
        store.ensure_agent(&id, "leader").unwrap();
        let mut control = Control::new(store, &id);
        assert!(
            control
                .submit(&TeamAction {
                    action_id: "input".into(),
                    session_id: id.clone(),
                    actor_id: "user".into(),
                    run_id: None,
                    kind: ActionKind::UserMessage,
                    payload: json!({"text":"PRESERVE_ORIGINAL_INPUT"}),
                })
                .unwrap()
                .ok
        );
        let run = control.store.runs_for_session(&id, &[TurnStatus::Queued]).unwrap().remove(0);
        let task: Task = serde_json::from_value(json!({
            "task_id":"prior-task","requester":"leader","assignee":"leader","description":"prior work","status":"SUCCEEDED"
        })).unwrap();
        control.store.insert_task(&id, &task).unwrap();
        let approval: ApprovalRequest = serde_json::from_value(json!({
            "approval_id":"prior-approval","session_id":id,"agent_id":"leader","run_id":run.run_id,
            "tool_call_id":"old-call","operation_hash":"old-operation","requested_scope":{},"policy_revision":1
        }))
        .unwrap();
        control.store.insert_approval(&approval).unwrap();
        let original: String = control
            .store
            .conn
            .query_row(&format!("SELECT {field} FROM {table} LIMIT 1"), [], |row| row.get(0))
            .unwrap();
        control.store.conn.execute(&format!("UPDATE {table} SET {field}=?1"), [bad]).unwrap();
        let before = snapshot(&control.store);
        let calls = api.calls();
        for initial_spec in [None, Some(spec.clone())] {
            let result = open_session(OpenOptions {
                cwd: Some(cwd.clone()),
                session_id: Some(id.clone()),
                catalog: Some(catalog.clone()),
                initial_spec,
                full_auto: true,
                ..Default::default()
            });
            let error = match result {
                Ok(opened) => {
                    opened.close();
                    panic!("{table}.{field}: corrupt persisted work was accepted");
                }
                Err(error) => error,
            };
            assert!(error.contains(field), "{table}.{field}: {error}");
            assert!(!error.contains("PRIVATE_BAD_VALUE"), "a diagnostic exposed private data");
            assert_eq!(snapshot(&control.store), before, "failed startup changed authoritative work");
            assert_eq!(api.calls(), calls, "failed startup requested the model");
            assert!(!is_session_locked(&id, None));
            assert!(!paths.base.join("members").exists(), "failed startup constructed a member");
            assert!(!cwd.join("report.txt").exists());
        }
        control.store.conn.execute(&format!("UPDATE {table} SET {field}=?1"), [original]).unwrap();
        control.store.expire_approval("prior-approval").unwrap();
        drop(control);
        let opened = open_session(OpenOptions {
            cwd: Some(cwd.clone()),
            session_id: Some(id.clone()),
            catalog: Some(catalog.clone()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(opened.core.state().unwrap()["session"]["permissions_mode"], "approved_scope");
        opened.runtime.start();
        assert!(opened.runtime.settle(10));
        let state = opened.core.state().unwrap();
        opened.close();
        assert_eq!(api.calls(), calls + 3, "{table}.{field}: the repaired run was not resumed exactly once");
        assert!(api.body(calls)["messages"].to_string().contains("PRESERVE_ORIGINAL_INPUT"));
        assert_eq!(state["events"].as_array().unwrap().iter().filter(|event| event["kind"] == "goal_done").count(), 1);
        assert_eq!(std::fs::read_to_string(cwd.join("report.txt")).unwrap(), "recovered once\n");
        assert_eq!(std::fs::read_to_string(cwd.join("sentinel.txt")).unwrap(), "original user file\n");
        assert_eq!(state["runs"].as_array().unwrap().len(), 1);
        assert_eq!(state["runs"][0]["run_id"], run.run_id);
    }
}

#[test]
fn task_requests_can_be_corrected_through_model_tools_and_survive_reopen() {
    for full_auto in [false, true] {
        let env = env_guard("task-request-correction");
        let cwd = env.join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let api = FakeOpenAi::start(|_, index| {
            (
                200,
                match index {
                    0 => {
                        tool_call_response("bad-assign", "assign_task", json!({"assignee":"leader","description":true}))
                    }
                    1 => tool_call_response(
                        "good-assign",
                        "assign_task",
                        json!({"assignee":"leader","task_id":"explicit-work","description":"Write a report",
                            "acceptance":"report.md contains verified"}),
                    ),
                    2 => tool_call_response(
                        "bad-complete",
                        "complete_task",
                        json!({"task_id":"explicit-work","summary":false}),
                    ),
                    3 => tool_call_response(
                        "write-report",
                        "write_file",
                        json!({"path":"report.md","content":"verified\n"}),
                    ),
                    4 => tool_call_response(
                        "good-complete",
                        "complete_task",
                        json!({"task_id":"explicit-work","summary":"verified","result_refs":["report.md"]}),
                    ),
                    _ => text_response("The report is ready."),
                },
            )
        });
        let opened = open_chat_session(&cwd, &api, &["files"], UserConfig::default());
        if full_auto {
            assert!(submit(&opened.core, "auto", "user", "set_permission_mode", json!({"mode":"full_auto"})).ok);
        }
        opened.runtime.start();
        opened.runtime.user_message("write and verify the report", false).unwrap();
        assert!(opened.runtime.settle(10));
        let state = opened.core.state().unwrap();
        opened.close();
        assert_eq!(state["tasks"].as_array().unwrap().len(), 1, "bad assign must not create a task: {state}");
        assert_eq!(state["tasks"][0]["status"], "SUCCEEDED", "{state}");
        assert_eq!(state["tasks"][0]["task_id"], "explicit-work");
        assert_eq!(state["tasks"][0]["result_refs"], json!(["report.md"]));
        assert_eq!(std::fs::read_to_string(cwd.join("report.md")).unwrap(), "verified\n");
        for (index, call_id, expected) in
            [(1, "bad-assign", false), (2, "good-assign", true), (3, "bad-complete", false), (5, "good-complete", true)]
        {
            let body = api.body(index);
            let result = body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|message| message["role"] == "tool" && message["tool_call_id"] == call_id)
                .unwrap_or_else(|| panic!("missing {call_id} in {body}"));
            let receipt: Json = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
            assert_eq!(receipt["error"].is_null(), expected, "{call_id}: {receipt}");
            if expected {
                assert_eq!(receipt["task_id"], "explicit-work", "{call_id}: {receipt}");
            } else {
                assert!(receipt["error"].as_str().is_some_and(|error| error.contains("任务参数无效")));
            }
        }
        let request = api.body(0);
        for name in ["assign_task", "complete_task", "wait_for_tasks", "cancel_task", "cancel_run"] {
            let tool =
                request["tools"].as_array().unwrap().iter().find(|tool| tool["function"]["name"] == name).unwrap();
            assert_eq!(tool["function"]["parameters"]["additionalProperties"], false);
        }
        let count = api.calls();
        drop(opened);
        let reopened = open_chat_session(&cwd, &api, &["files"], UserConfig::default());
        let restored = reopened.core.state().unwrap();
        reopened.close();
        assert_eq!(restored["tasks"], state["tasks"]);
        assert_eq!(api.calls(), count, "opening the session must not replay the completed request");
        assert_eq!(
            restored["events"].as_array().unwrap().iter().filter(|event| event["kind"] == "task_completed").count(),
            1
        );
    }
}

#[test]
fn communication_tools_correct_refusals_preserve_shared_pages_and_finish_after_reopen() {
    for full_auto in [false, true] {
        let env = env_guard("communication-correction");
        let cwd = env.join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let api = FakeOpenAi::start(|_, index| {
            (
                200,
                match index {
                    0 => tool_call_response("bad-publish", "publish_shared", json!({"space_id":"one","content":true})),
                    1 => {
                        tool_call_response("publish-one", "publish_shared", json!({"space_id":"one","content":"first"}))
                    }
                    2 => tool_call_response(
                        "publish-two",
                        "publish_shared",
                        json!({"space_id":"two","content":"second"}),
                    ),
                    3 => tool_call_response("bad-read", "read_shared", json!({"space_id":false})),
                    4 => tool_call_response("read-one", "read_shared", json!({"space_id":"one","limit":1})),
                    5 => tool_call_response("read-next", "read_shared", json!({"limit":1})),
                    6 => tool_call_response("bad-done", "signal_done", json!({"summary":false})),
                    7 => tool_call_response(
                        "report",
                        "write_file",
                        json!({"path":"report.md","content":"checked two entries\n"}),
                    ),
                    8 => tool_call_response("done", "signal_done", json!({"summary":"verified"})),
                    _ => text_response("Verified both shared entries and the report."),
                },
            )
        });
        let mut catalog = UserConfig::default();
        catalog.models.insert("m".into(), profile(&api.base_url(), "openai", json!({}), 0));
        let spec = json!({
            "leader_id":"leader",
            "agents":[agent_json("leader","leader",&["files"])],
            "shared_spaces":[
                {"id":"one","readers":["leader"],"writers":["leader"]},
                {"id":"two","readers":["leader"],"writers":["leader"]}
            ]
        });
        let open = || {
            open_session(OpenOptions {
                cwd: Some(cwd.clone()),
                session_id: Some("communication".into()),
                initial_spec: Some(spec.clone()),
                catalog: Some(catalog.clone()),
                ..Default::default()
            })
            .unwrap()
        };
        let opened = open();
        if full_auto {
            assert!(submit(&opened.core, "auto", "user", "set_permission_mode", json!({"mode":"full_auto"})).ok);
        }
        opened.runtime.start();
        opened.runtime.user_message("Check the shared findings and write a report.", false).unwrap();
        assert!(opened.runtime.settle(15));
        let state = opened.core.state().unwrap();
        opened.close();
        assert_eq!(std::fs::read_to_string(cwd.join("report.md")).unwrap(), "checked two entries\n");
        assert_eq!(state["events"].as_array().unwrap().iter().filter(|e| e["kind"] == "goal_done").count(), 1);
        for (index, call_id, expected) in [
            (1, "bad-publish", false),
            (2, "publish-one", true),
            (3, "publish-two", true),
            (4, "bad-read", false),
            (5, "read-one", true),
            (6, "read-next", true),
            (7, "bad-done", false),
            (9, "done", true),
        ] {
            let body = api.body(index);
            let result = body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|message| message["role"] == "tool" && message["tool_call_id"] == call_id)
                .unwrap();
            let receipt: Json = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
            assert_eq!(receipt["error"].is_null(), expected, "{call_id}: {receipt}");
            if call_id == "read-one" || call_id == "read-next" {
                assert_eq!(receipt["entries"].as_array().unwrap().len(), 1);
                assert_eq!(receipt["entries"][0]["content"], if call_id == "read-one" { "first" } else { "second" });
            }
        }
        let body = api.body(0);
        for name in ["send_message", "publish_shared", "read_shared", "list_shared", "request_help", "signal_done"] {
            let tool = body["tools"].as_array().unwrap().iter().find(|tool| tool["function"]["name"] == name).unwrap();
            assert_eq!(tool["function"]["parameters"]["additionalProperties"], false);
            if name == "publish_shared" {
                assert_eq!(tool["function"]["parameters"]["properties"]["supersedes"]["type"], "string");
            }
        }
        let calls = api.calls();
        drop(opened);
        let restored = open();
        assert_eq!(api.calls(), calls);
        let page = submit(&restored.core, "after-reopen", "leader", "read_shared", json!({}));
        let spaces = submit(&restored.core, "list-after-reopen", "leader", "list_shared", json!({}));
        restored.close();
        assert!(page.ok && spaces.ok);
        assert_eq!(page.result["entries"], json!([]));
        assert!(spaces.result["spaces"].as_array().unwrap().iter().all(|space| space["entries"] == 1));
        assert_eq!(api.calls(), calls, "reopening and reading must not repeat the model turn");
    }
}

#[test]
fn review_topology_update_must_rebuild_runner() {
    let _env = env_guard("review-update");
    let cwd = isolated_project("update");
    let api = FakeOpenAi::start(|_, _| (200, text_response("done")));
    let opened = open_chat_session(&cwd, &api, &["files"], UserConfig::default());
    opened.runtime.start();
    opened.runtime.user_message("first", false).unwrap();
    assert!(opened.runtime.settle(5));
    assert_eq!(api.body(0)["model"], "test");
    let receipt = submit(
        &opened.core,
        "update-model",
        "leader",
        "apply_topology_patch",
        json!({
            "base_revision":1, "operations":[{"op":"update_agent", "agent_id":"leader",
            "changes":{"model_profile":"other", "instructions":"NEW INSTRUCTIONS", "tool_bindings":[]}}]
        }),
    );
    assert!(receipt.ok, "{receipt:?}");
    assert_eq!(receipt.result["status"], "APPLIED");
    opened.runtime.user_message("second", false).unwrap();
    assert!(opened.runtime.settle(5));
    let second = api.body(1);
    opened.close();
    eprintln!("spec updated to other, actual request model={}", second["model"]);
    assert_eq!(second["model"], "new-model", "APPLIED patch left the old model active");
    assert!(second["messages"][0]["content"].as_str().unwrap().contains("NEW INSTRUCTIONS"));
    assert!(!second["tools"].as_array().unwrap().iter().any(|t| t["function"]["name"] == "write_file"));
}

/// D-30: add_agent without a model profile auto-creates a member-named session
/// profile cloned from the Leader's effective config; an unknown non-empty
/// value is treated as a requested model id on the Leader's connection.
/// The session profile survives a reopen.
#[test]
fn review_add_agent_inherits_leader_tools_and_gets_channels() {
    let _env = env_guard("review-addagent-d33");
    let cwd = isolated_project("d33");
    let api = FakeOpenAi::start(|_, index| {
        (
            200,
            if index == 0 {
                tool_call_response(
                    "topo-1",
                    "apply_topology_patch",
                    json!({
                        "base_revision": 1,
                        "operations": [
                            // no tool_bindings: the member must inherit the Leader's
                            {"op":"add_agent","agent":{"id":"w1","name":"W1","role":"worker","runtime_kind":"deepagents"}},
                            // explicit [] stays messaging-only
                            {"op":"add_agent","agent":{"id":"w2","name":"W2","role":"worker","runtime_kind":"deepagents","tool_bindings":[]}},
                        ]
                    }),
                )
            } else {
                text_response("done")
            },
        )
    });
    let opened = open_chat_session(&cwd, &api, &["files", "shell"], UserConfig::default());
    opened.runtime.start();
    opened.runtime.user_message("build a team", false).unwrap();
    assert!(opened.runtime.settle(5));

    let state = opened.core.state().unwrap();
    let agents = state["spec"]["agents"].as_array().cloned().unwrap_or_default();
    let tools_of = |id: &str| agents.iter().find(|a| a["id"] == id).map(|a| a["tool_bindings"].clone());
    assert_eq!(tools_of("w1"), Some(json!(["files", "shell"])), "an omitted list inherits the Leader's: {state}");
    assert_eq!(tools_of("w2"), Some(json!([])), "an explicit empty list is respected: {state}");

    // both directions exist, so delegation reports and instructions can travel
    let spec: teamagents_core::models::TeamSpec = serde_json::from_value(state["spec"].clone()).unwrap();
    assert!(spec.can_send("w1", "leader"), "member -> leader: {state}");
    assert!(spec.can_send("leader", "w1"), "leader -> member: {state}");
    assert!(!spec.can_send("w1", "w2") && !spec.can_send("w2", "w1"), "members do not message each other: {state}");
    opened.close();
}

#[test]
fn review_add_agent_auto_creates_member_profile() {
    let _env = env_guard("review-autoprofile");
    let cwd = isolated_project("autoprofile");
    let api = FakeOpenAi::start(|_, index| {
        (
            200,
            if index == 0 {
                tool_call_response(
                    "topo-1",
                    "apply_topology_patch",
                    json!({
                        "base_revision": 1,
                        "operations": [
                            {"op":"add_agent","agent":{"id":"w1","name":"W1","role":"worker",
                                "runtime_kind":"deepagents","tool_bindings":[]}},
                            {"op":"add_agent","agent":{"id":"w2","name":"W2","role":"worker",
                                "runtime_kind":"deepagents","model_profile":"gpt-9","tool_bindings":[]}},
                        ]
                    }),
                )
            } else {
                text_response("done")
            },
        )
    });
    let opened = open_chat_session(&cwd, &api, &["files"], UserConfig::default());
    opened.runtime.start();
    opened.runtime.user_message("build a team", false).unwrap();
    assert!(opened.runtime.settle(5));

    // both ops were rewritten to member-named session profiles before submit
    let state = opened.core.state().unwrap();
    let agents = state["spec"]["agents"].as_array().cloned().unwrap_or_default();
    let profile_of = |id: &str| agents.iter().find(|a| a["id"] == id).map(|a| a["model_profile"].clone());
    assert_eq!(profile_of("w1"), Some(json!("w1")), "{state}");
    assert_eq!(profile_of("w2"), Some(json!("w2")), "{state}");

    // profiles.json: w1 cloned the leader's model; w2 carries the requested model id
    let dir = std::env::var("XDG_STATE_HOME").unwrap();
    let text = std::fs::read_to_string(format!("{dir}/teamagents/sessions/review/profiles.json")).unwrap();
    let profiles: Json = serde_json::from_str(&text).unwrap();
    assert_eq!(profiles["w1"]["model"], json!("test"), "{profiles}");
    assert_eq!(profiles["w1"]["base_url"], json!(api.base_url()));
    assert_eq!(profiles["w2"]["model"], json!("gpt-9"), "{profiles}");

    // the report resolves member-named profiles through the session overlay
    let report = opened.model_report();
    let agents = report["agents"].as_array().cloned().unwrap_or_default();
    let model_of = |id: &str| agents.iter().find(|a| a["agent_id"] == id).map(|a| a["model"].clone());
    assert_eq!(model_of("w1"), Some(json!("test")), "{report}");
    assert_eq!(model_of("w2"), Some(json!("gpt-9")), "{report}");
    assert!(report["profiles"].as_array().unwrap().iter().any(|p| p["id"] == "w1"), "{report}");
    opened.close();

    // reopen: the session overlay still resolves (D-30 persistence)
    let reopened = open_chat_session(&cwd, &api, &["files"], UserConfig::default());
    let report = reopened.model_report();
    let agents = report["agents"].as_array().cloned().unwrap_or_default();
    let model_of = |id: &str| agents.iter().find(|a| a["agent_id"] == id).map(|a| a["model"].clone());
    assert_eq!(model_of("w1"), Some(json!("test")), "{report}");
    assert_eq!(model_of("w2"), Some(json!("gpt-9")), "{report}");
    reopened.close();
}

#[test]
fn topology_nonleader_cannot_change_profiles_before_core_refuses_the_patch() {
    let env = env_guard("topology-nonleader");
    let cwd = env.join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let api = FakeOpenAi::start(|_, index| {
        (
            200,
            if index == 0 {
                tool_call_response(
                    "unauthorized-patch",
                    "apply_topology_patch",
                    json!({
                        "base_revision": 2, "operations": [{"op":"add_agent", "agent":{
                            "id":"m", "name":"M", "role":"worker", "runtime_kind":"deepagents",
                            "model_profile":"forbidden-model"
                        }}]
                    }),
                )
            } else {
                text_response("done")
            },
        )
    });
    let opened = open_chat_session(&cwd, &api, &[], UserConfig::default());
    assert!(
        submit(
            &opened.core,
            "add-worker",
            "leader",
            "apply_topology_patch",
            json!({
                "base_revision":1, "operations":[{"op":"add_agent", "agent":agent_json("worker", "worker", &[])}]
            })
        )
        .ok
    );
    assert!(
        submit(
            &opened.core,
            "worker-task",
            "leader",
            "assign_task",
            json!({
                "assignee":"worker", "description":"Report an invalid topology request."
            })
        )
        .ok
    );
    opened.runtime.start();
    assert!(opened.runtime.settle(10));
    let state = opened.core.state().unwrap();
    let selected =
        opened.model_report()["agents"].as_array().unwrap().iter().find(|agent| agent["agent_id"] == "leader").unwrap()
            ["model"]
            .clone();
    let profiles_path = env.join("teamagents/sessions/review/profiles.json");
    let wrote_profiles = profiles_path.exists();
    opened.close();

    let receipt = api.body(1)["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["tool_call_id"] == "unauthorized-patch")
        .unwrap()["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(receipt.contains("only the Leader"), "the core must refuse the actor: {receipt}");
    assert_eq!(state["revision"], 2);
    assert_eq!(selected, "test", "a refused worker request replaced the Leader's model");
    assert!(!wrote_profiles, "a non-Leader must never prepare persistent model profiles");

    let reopened = open_chat_session(&cwd, &api, &[], UserConfig::default());
    let before = api.calls();
    reopened.runtime.start();
    reopened.runtime.user_message("continue as Leader", false).unwrap();
    assert!(reopened.runtime.settle(10));
    reopened.close();
    assert_eq!(api.body(before)["model"], "test", "reopening must keep the authorized connection");
}

#[test]
fn topology_duplicate_member_refusal_preserves_profiles_and_allows_correction() {
    let env = env_guard("topology-duplicate-profile");
    let cwd = env.join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let api = FakeOpenAi::start(|_, index| {
        (
            200,
            match index {
                0 => tool_call_response(
                    "duplicate-member",
                    "apply_topology_patch",
                    json!({
                        "base_revision":2, "operations":[{"op":"add_agent", "agent":{
                            "id":"m", "name":"Duplicate", "role":"worker", "runtime_kind":"deepagents",
                            "model_profile":"forbidden-model"
                        }}]
                    }),
                ),
                1 => tool_call_response(
                    "corrected-member",
                    "apply_topology_patch",
                    json!({
                        "base_revision":2, "operations":[{"op":"add_agent", "agent":{
                            "id":"reviewer", "name":"Reviewer", "role":"worker", "runtime_kind":"deepagents",
                            "model_profile":"review-model"
                        }}]
                    }),
                ),
                _ => text_response("done"),
            },
        )
    });
    let opened = open_chat_session(&cwd, &api, &[], UserConfig::default());
    assert!(
        submit(
            &opened.core,
            "existing-member",
            "leader",
            "apply_topology_patch",
            json!({
                "base_revision":1, "operations":[{"op":"add_agent", "agent":agent_json("m", "worker", &[])}]
            })
        )
        .ok
    );
    opened.runtime.start();
    opened.runtime.user_message("build a team and correct invalid proposals", false).unwrap();
    assert!(opened.runtime.settle(10));
    let selected =
        opened.model_report()["agents"].as_array().unwrap().iter().find(|agent| agent["agent_id"] == "leader").unwrap()
            ["model"]
            .clone();
    let state = opened.core.state().unwrap();
    opened.close();

    let profiles: Json =
        serde_json::from_str(&std::fs::read_to_string(env.join("teamagents/sessions/review/profiles.json")).unwrap())
            .unwrap();
    assert_eq!(selected, "test", "rejected duplicate identity poisoned a referenced profile");
    assert!(profiles.get("m").is_none(), "the rejected request must not shadow user configuration: {profiles}");
    assert_eq!(profiles["reviewer"]["model"], "review-model");
    assert_eq!(state["revision"], 3, "only the corrected request changes the team");
    assert_eq!(state["spec"]["agents"].as_array().unwrap().len(), 3);

    let reopened = open_chat_session(&cwd, &api, &[], UserConfig::default());
    let before = api.calls();
    reopened.runtime.start();
    reopened.runtime.user_message("continue", false).unwrap();
    assert!(reopened.runtime.settle(10));
    let reviewer = reopened.model_report()["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["agent_id"] == "reviewer")
        .unwrap()["model"]
        .clone();
    reopened.close();
    assert_eq!(api.body(before)["model"], "test");
    assert_eq!(reviewer, "review-model");
}

#[test]
fn topology_prepare_failure_preserves_memory_catalog_and_allows_retry() {
    for storage_failure in [false, true] {
        let env = env_guard("topology-prepare-failure");
        let cwd = env.join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let member = json!({"op":"add_agent", "agent":{
            "id":"reviewer", "name":"Reviewer", "role":"worker", "runtime_kind":"deepagents",
            "model_profile":"review-model"
        }});
        let mut operations = vec![member.clone()];
        if !storage_failure {
            operations.push(json!({"op":"add_agent", "agent":{
                "id":"invalid", "name":"Invalid", "role":"worker", "runtime_kind":"deepagents",
                "model_profile":false
            }}));
        }
        let api = FakeOpenAi::start(move |_, index| {
            (
                200,
                match index {
                    0 => tool_call_response(
                        "failed-prepare",
                        "apply_topology_patch",
                        json!({
                            "base_revision":1, "operations":operations
                        }),
                    ),
                    2 => tool_call_response(
                        "retry-prepare",
                        "apply_topology_patch",
                        json!({
                            "base_revision":1, "operations":[member]
                        }),
                    ),
                    _ => text_response("done"),
                },
            )
        });
        let opened = open_chat_session(&cwd, &api, &[], UserConfig::default());
        let path = env.join("teamagents/sessions/review/profiles.json");
        if storage_failure {
            // A directory at the staging path deterministically rejects the write.
            std::fs::create_dir(path.with_extension("tmp")).unwrap();
        }
        opened.runtime.start();
        opened.runtime.user_message("try the invalid patch", false).unwrap();
        assert!(opened.runtime.settle(10));
        assert_eq!(api.calls(), 2);
        assert_eq!(opened.core.state().unwrap()["revision"], 1);
        assert!(!path.exists(), "failed preparation must not publish the valid prefix");
        assert!(!opened.model_report()["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|profile| profile["id"] == "reviewer"));
        let receipt = submit(
            &opened.core,
            "probe-catalog",
            "leader",
            "apply_topology_patch",
            json!({
                "base_revision":1, "operations":[{"op":"update_agent", "agent_id":"leader", "changes":{
                    "model_profile":"reviewer"
                }}]
            }),
        );
        assert!(!receipt.ok, "failed preparation leaked a profile into the core catalog: {receipt:?}");
        assert!(receipt.error.unwrap().contains("unknown model profile"));
        let error = api.body(1)["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["tool_call_id"] == "failed-prepare")
            .unwrap()["content"]
            .to_string();
        assert!(error.contains(if storage_failure { "persist session profiles" } else { "必须是字符串" }), "{error}");
        if storage_failure {
            std::fs::remove_dir(path.with_extension("tmp")).unwrap();
        }
        opened.runtime.user_message("retry the corrected patch", false).unwrap();
        assert!(opened.runtime.settle(10));
        assert_eq!(opened.core.state().unwrap()["revision"], 2);
        opened.close();
        let profiles: Json = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(profiles["reviewer"]["model"], "review-model");
        assert!(profiles.get("invalid").is_none());
    }
}

#[test]
fn topology_unused_profile_can_be_corrected_but_referenced_profile_cannot_be_replaced() {
    let env = env_guard("topology-unused-profile");
    let cwd = env.join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let api = FakeOpenAi::start(|_, index| {
        (
            200,
            if index < 3 {
                tool_call_response(
                    &format!("patch-{index}"),
                    "apply_topology_patch",
                    json!({
                        "base_revision": if index == 2 { 2 } else { 1 },
                        "operations":[{"op":"add_agent", "agent":{
                            "id":"reviewer", "name":"Reviewer", "runtime_kind":"deepagents",
                            "role": if index == 0 { "leader" } else { "worker" },
                            "model_profile": (["bad-model", "review-model", "forbidden-model"][index])
                        }}]
                    }),
                )
            } else {
                text_response("done")
            },
        )
    });
    let opened = open_chat_session(&cwd, &api, &[], UserConfig::default());
    opened.runtime.start();
    opened.runtime.user_message("correct the rejected profile and preserve its final selection", false).unwrap();
    assert!(opened.runtime.settle(10));
    assert_eq!(api.calls(), 4);
    assert_eq!(opened.core.state().unwrap()["revision"], 2);
    opened.close();
    let profiles: Json =
        serde_json::from_str(&std::fs::read_to_string(env.join("teamagents/sessions/review/profiles.json")).unwrap())
            .unwrap();
    assert_eq!(profiles["reviewer"]["model"], "review-model", "an unused residue must not pin the rejected model");
    let reopened = open_chat_session(&cwd, &api, &[], UserConfig::default());
    let report = reopened.model_report();
    assert_eq!(
        report["agents"].as_array().unwrap().iter().find(|agent| agent["agent_id"] == "reviewer").unwrap()["model"],
        "review-model"
    );
    reopened.close();
}

#[test]
fn topology_profile_name_collision_allows_explicit_existing_profile() {
    let env = env_guard("topology-profile-collision");
    let cwd = env.join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let api = FakeOpenAi::start(|_, index| {
        (
            200,
            if index < 2 {
                tool_call_response(
                    &format!("collision-{index}"),
                    "apply_topology_patch",
                    json!({
                        "base_revision":1, "operations":[{"op":"add_agent", "agent":{
                            "id":"other", "name":"Other", "role":"worker", "runtime_kind":"deepagents",
                            "model_profile": if index == 0 { "forbidden-model" } else { "other" }
                        }}]
                    }),
                )
            } else {
                text_response("done")
            },
        )
    });
    let opened = open_chat_session(&cwd, &api, &[], UserConfig::default());
    opened.runtime.start();
    opened.runtime.user_message("use the existing profile explicitly", false).unwrap();
    assert!(opened.runtime.settle(10));
    assert_eq!(api.calls(), 3);
    assert_eq!(opened.core.state().unwrap()["revision"], 2);
    let report = opened.model_report();
    assert_eq!(
        report["agents"].as_array().unwrap().iter().find(|agent| agent["agent_id"] == "other").unwrap()["model"],
        "new-model"
    );
    opened.close();
    assert!(!env.join("teamagents/sessions/review/profiles.json").exists());
}

#[test]
fn topology_request_invalid_or_stale_envelope_never_prepares_profiles() {
    for (key, value) in [
        ("base_revision", None),
        ("base_revision", Some(json!(0))),
        ("base_revision", Some(json!("1"))),
        ("base_revision", Some(Json::Null)),
        ("reject", Some(json!(true))),
        ("reject", Some(json!("false"))),
        ("reject", Some(Json::Null)),
        ("patch_id", Some(json!(7))),
        ("patch_id", Some(Json::Null)),
        ("patch_id", Some(json!("missing-patch"))),
        ("unknown", Some(json!(true))),
    ] {
        let env = env_guard("topology-invalid-envelope");
        let cwd = env.join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let valid = json!({"base_revision":1, "operations":[{"op":"add_agent", "agent":{
            "id":"reviewer", "name":"Reviewer", "role":"worker", "runtime_kind":"deepagents"
        }}]});
        let mut invalid = valid.clone();
        if let Some(value) = value {
            invalid[key] = value;
        } else {
            invalid.as_object_mut().unwrap().remove(key);
        }
        let api = FakeOpenAi::start(move |_, index| {
            (
                200,
                match index {
                    0 => tool_call_response("invalid-envelope", "apply_topology_patch", invalid.clone()),
                    2 => tool_call_response("valid-envelope", "apply_topology_patch", valid.clone()),
                    _ => text_response("done"),
                },
            )
        });
        let opened = open_chat_session(&cwd, &api, &[], UserConfig::default());
        opened.runtime.start();
        opened.runtime.user_message("try an invalid request", false).unwrap();
        assert!(opened.runtime.settle(10));
        let state = opened.core.state().unwrap();
        let report = opened.model_report();
        opened.close();
        assert_eq!(api.calls(), 2);
        assert_eq!(state["revision"], 1, "{key}: {state}");
        assert!(
            !env.join("teamagents/sessions/review/profiles.json").exists(),
            "{key}: invalid request persisted profiles"
        );
        assert!(
            !report["profiles"].as_array().unwrap().iter().any(|profile| profile["id"] == "reviewer"),
            "{key}: {report}"
        );

        let reopened = open_chat_session(&cwd, &api, &[], UserConfig::default());
        reopened.runtime.start();
        reopened.runtime.user_message("retry the corrected request", false).unwrap();
        assert!(reopened.runtime.settle(10));
        let state = reopened.core.state().unwrap();
        reopened.close();
        assert_eq!(state["revision"], 2, "corrected request after {key} must apply: {state}");
        assert!(state["spec"]["agents"].as_array().unwrap().iter().any(|agent| agent["id"] == "reviewer"));
    }
}

#[test]
fn topology_request_stored_proposal_inherits_leader_defaults_when_applied() {
    for empty_operations in [false, true] {
        let env = env_guard("topology-stored-defaults");
        let cwd = env.join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let decision = Arc::new(Mutex::new(Json::Null));
        let response = decision.clone();
        let api = FakeOpenAi::start(move |_, index| {
            (
                200,
                if index == 0 {
                    tool_call_response("accept-proposal", "apply_topology_patch", response.lock().unwrap().clone())
                } else {
                    text_response("done")
                },
            )
        });
        let opened = open_chat_session(&cwd, &api, &["files"], UserConfig::default());
        assert!(
            submit(
                &opened.core,
                "add-b",
                "leader",
                "apply_topology_patch",
                json!({
                    "base_revision":1, "operations":[{"op":"add_agent", "agent":agent_json("b", "worker", &[])}]
                })
            )
            .ok
        );
        let proposed = submit(
            &opened.core,
            "propose-reviewer",
            "b",
            "propose_team_change",
            json!({
                "operations":[{"op":"add_agent", "agent":{
                    "id":"reviewer", "name":"Reviewer", "role":"worker", "runtime_kind":"deepagents"
                }}], "rationale":"need a reviewer"
            }),
        );
        assert!(proposed.ok, "{proposed:?}");
        let mut payload = json!({"patch_id":proposed.result["patch_id"]});
        if empty_operations {
            payload["operations"] = json!([]);
        }
        *decision.lock().unwrap() = payload;
        opened.runtime.start();
        opened.runtime.user_message("accept the member proposal", false).unwrap();
        assert!(opened.runtime.settle(10));
        let state = opened.core.state().unwrap();
        let report = opened.model_report();
        opened.close();
        assert_eq!(
            state["revision"], 3,
            "stored proposal must receive the same preparation as an inline patch: {state}"
        );
        let reviewer =
            state["spec"]["agents"].as_array().unwrap().iter().find(|agent| agent["id"] == "reviewer").unwrap();
        assert_eq!(reviewer["model_profile"], "reviewer");
        assert_eq!(reviewer["tool_bindings"], json!(["files"]));
        for (source, target) in [("leader", "reviewer"), ("reviewer", "leader")] {
            assert!(state["spec"]["channels"].as_array().unwrap().iter().any(|channel| channel["source"] == source
                && channel["mode"] == "message"
                && channel["targets"].as_array().unwrap().contains(&json!(target))));
        }
        assert_eq!(
            report["agents"].as_array().unwrap().iter().find(|agent| agent["agent_id"] == "reviewer").unwrap()["model"],
            "test"
        );
        assert!(state["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["kind"] == "topology_proposed" && event["payload"]["proposer"] == "b"));
        let reopened = open_chat_session(&cwd, &api, &["files"], UserConfig::default());
        let report = reopened.model_report();
        reopened.close();
        assert_eq!(
            report["agents"].as_array().unwrap().iter().find(|agent| agent["agent_id"] == "reviewer").unwrap()["model"],
            "test"
        );
    }
}

#[test]
fn review_bound_mcp_must_be_advertised_to_model() {
    let _env = env_guard("review-mcp");
    let cwd = isolated_project("mcp");
    let api = FakeOpenAi::start(|_, index| {
        (
            200,
            if index == 0 {
                tool_call_response("mcp-1", "echo_echo", json!({"text":"ping"}))
            } else {
                text_response("done")
            },
        )
    });
    let mut catalog = UserConfig::default();
    catalog.tools.insert(
        "echo_service".into(),
        serde_json::from_value(json!({
            "kind":"mcp", "mcp_server":"echo", "mcp_execution":"host", "command":env!("CARGO_BIN_EXE_fake-mcp-server"),
            "tool_names":["echo"], "required":true
        }))
        .unwrap(),
    );
    let bound = BoundTools::load(&catalog, &["echo_service".to_string()]).unwrap();
    assert!(bound.names().contains("echo_echo"));
    assert_eq!(bound.call("echo_echo", &json!({"text":"ping"})).unwrap().unwrap(), json!("ping"));
    bound.close();
    let opened = open_chat_session(&cwd, &api, &["echo_service"], catalog);
    opened.runtime.start();
    opened.runtime.user_message("use echo", false).unwrap();
    assert!(opened.runtime.settle(5));
    let body = api.body(0);
    opened.close();
    assert!(api.body(1)["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "tool" && m["content"].as_str().unwrap_or("").contains("ping")));
    let names: Vec<&str> =
        body["tools"].as_array().unwrap().iter().filter_map(|t| t["function"]["name"].as_str()).collect();
    eprintln!("actual model tools={names:?}");
    assert!(names.contains(&"echo_echo"), "loaded and callable MCP tool vanished from the model request");
}

#[test]
fn review_closed_session_must_not_execute_late_tool_call() {
    let _env = env_guard("review-close");
    let cwd = isolated_project("close");
    let api = FakeOpenAi::start(|_, index| {
        if index == 0 {
            std::thread::sleep(Duration::from_millis(400));
            (200, tool_call_response("late", "write_file", json!({"path":"after-close.txt", "content":"late write"})))
        } else {
            (200, text_response("done"))
        }
    });
    let opened = open_chat_session(&cwd, &api, &["files"], UserConfig::default());
    opened.runtime.start();
    opened.runtime.user_message("wait", false).unwrap();
    assert!(wait_for(|| api.calls() == 1, 5000));
    opened.close();
    let lock = teamagents_engine::sessions::acquire_session_lock("review").expect("close released the lock");
    let events_after_close = opened.core.state().unwrap()["events"].clone();
    let wrote = wait_for(|| cwd.join("after-close.txt").exists(), 2000);
    assert_eq!(
        opened.core.state().unwrap()["events"],
        events_after_close,
        "old runtime finalized after releasing ownership"
    );
    drop(lock);
    eprintln!("session lock was released; post-close write={wrote}");
    assert!(!wrote, "old runner still executed a model tool call after close returned");
    let resumed = open_chat_session(&cwd, &api, &["files"], UserConfig::default());
    resumed.runtime.start();
    assert!(resumed.runtime.settle(5), "checkpoint should resume on a new runtime");
    assert!(!cwd.join("after-close.txt").exists());
    resumed.close();
}

struct ProbeWorker {
    child: std::process::Child,
    input: std::process::ChildStdin,
    replies: std::sync::mpsc::Receiver<Json>,
    next_id: u64,
}
impl ProbeWorker {
    fn spawn(base: &std::path::Path) -> Self {
        use std::io::BufRead;
        use std::process::{Command, Stdio};
        let mut child = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .arg("serve")
            .env("XDG_CONFIG_HOME", base.join("config"))
            .env("XDG_STATE_HOME", base.join("state"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, replies) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
                let message: Json = serde_json::from_str(&line).unwrap();
                if message.get("id").is_some() {
                    let _ = tx.send(message);
                }
            }
        });
        Self { child, input, replies, next_id: 0 }
    }
    fn call(&mut self, method: &str, params: Json) -> Result<Json, String> {
        self.next_id += 1;
        writeln!(self.input, "{}", json!({"id":self.next_id,"method":method,"params":params})).unwrap();
        self.input.flush().unwrap();
        let r = self.replies.recv_timeout(Duration::from_secs(3)).map_err(|e| format!("{method}: {e}"))?;
        assert_eq!(r["id"], self.next_id);
        if let Some(err) = r.get("error") {
            Err(err.to_string())
        } else {
            Ok(r["result"].clone())
        }
    }
    fn entries(&mut self) -> Json {
        self.call("call", json!({"method":"shared_entries", "params":{"space_ids":["main"]}})).unwrap()["entries"]
            .clone()
    }
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Drop for ProbeWorker {
    fn drop(&mut self) {
        self.kill();
    }
}

fn worker_files(base: &std::path::Path, api: Option<&FakeOpenAi>) -> std::path::PathBuf {
    std::fs::create_dir_all(base.join("config/teamagents")).unwrap();
    std::fs::create_dir_all(base.join("project")).unwrap();
    let mut config = "[models.m]\nprovider='openai'\nprotocol='openai'\nmodel='test'\nmax_retries=0\n".to_string();
    if let Some(api) = api {
        config.push_str(&format!("base_url='{}'\n", api.base_url()));
    }
    std::fs::write(base.join("config/teamagents/config.toml"), config).unwrap();
    let team = base.join("team.json");
    std::fs::write(
        &team,
        json!({"leader_id":"leader", "agents":[agent_json("leader", "leader", &["files"])],
        "shared_spaces":[{"id":"main", "readers":["leader"], "writers":["leader"]}]})
        .to_string(),
    )
    .unwrap();
    team
}

#[test]
#[ignore = "v1 后端随 R29 退役（teamagents exec 现为 v2 无头客户端）；等价覆盖：v2_driver::user_cancel_stops_a_running_job、jobs_runner::a_successful_commands_service_outlives_the_job（A12/D-41）、v2_driver 崩溃恢复组"]
fn review_crash_after_committed_chat_action_must_not_replay_it() {
    let _env = env_guard("review-recovery");
    for lost_receipt in [false, true] {
        let base = isolated_project("recovery");
        let api = FakeOpenAi::start(|body, index| {
            // The model must see the committed result after restart. An API that
            // deliberately requests the same operation again is a different task.
            if index == 1 {
                std::thread::sleep(Duration::from_millis(500));
            }
            let has_result = body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["role"] == "tool" && m["content"].as_str().unwrap_or("").contains("sequence"));
            (
                200,
                if has_result {
                    text_response("done")
                } else {
                    tool_call_response(
                        &format!("publish-{index}"),
                        "publish_shared",
                        json!({"space_id":"main", "content":"once-only"}),
                    )
                },
            )
        });
        let team = worker_files(&base, Some(&api));
        let mut worker = ProbeWorker::spawn(&base);
        let opened = worker.call("open", json!({"cwd":base.join("project"),"team":team})).unwrap();
        worker.call("user_message", json!({"text":"publish once"})).unwrap();
        assert!(wait_for(|| api.calls() >= 2, 5000));
        assert_eq!(worker.entries().as_array().unwrap().len(), 1);
        let state = worker.call("call", json!({"method":"state", "params":{}})).unwrap();
        let run_id = state["runs"][0]["run_id"].as_str().unwrap();
        worker.kill();
        if lost_receipt {
            // Inject exactly the commit -> receipt-checkpoint crash window, using
            // the real model ID and already committed SQLite action receipt.
            let path = base
                .join("state/teamagents/sessions")
                .join(opened["session_id"].as_str().unwrap())
                .join("members/leader/turns")
                .join(format!("{run_id}.json"));
            let mut checkpoint: Json = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(checkpoint["history"].as_array_mut().unwrap().pop().unwrap()["role"], "tool");
            checkpoint["model_steps"] = json!(1);
            std::fs::write(&path, checkpoint.to_string()).unwrap();
            // The actual pre-result boundary has no result in either file.
            let history_path = path.parent().unwrap().parent().unwrap().join("chat_history.json");
            let mut history: Json = serde_json::from_slice(&std::fs::read(&history_path).unwrap()).unwrap();
            history[state["runs"][0]["context_ref"].as_str().unwrap()] = checkpoint["history"].clone();
            std::fs::write(history_path, history.to_string()).unwrap();
        }
        let mut worker = ProbeWorker::spawn(&base);
        worker.call("open", json!({"cwd":base.join("project"),"team":team,"resume":opened["session_id"]})).unwrap();
        assert!(wait_for(|| api.calls() >= 3, 5000));
        assert!(
            api.body(2)["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["role"] == "tool" && m["tool_call_id"] == "publish-0"),
            "recovery lost the original tool result"
        );
        let entries = worker.entries();
        eprintln!("entries after kill/restart: {entries}");
        assert_eq!(
            entries.as_array().unwrap().len(),
            1,
            "a committed model action was replayed with a new tool_call_id"
        );
    }
}

#[test]
fn review_close_must_stop_running_shell_before_unlocking() {
    let _env = env_guard("review-shell-close");
    if !teamagents_engine::tools::bwrap_available() {
        eprintln!("skipped: bwrap is unavailable, cannot exercise real shell isolation");
        return;
    }
    let cwd = isolated_project("shell-close");
    let api = FakeOpenAi::start(|_, _| {
        (
            200,
            tool_call_response(
                "shell-close",
                "shell",
                json!({
                    "command":"touch started; sleep 2; printf late > after-close.txt", "timeout":10
                }),
            ),
        )
    });
    let opened = open_chat_session(&cwd, &api, &["shell"], UserConfig::default());
    opened.runtime.start();
    opened.runtime.user_message("run command", false).unwrap();
    assert!(wait_for(|| cwd.join("started").exists(), 5000), "real sandbox never executed");
    opened.close();
    let _lock = teamagents_engine::sessions::acquire_session_lock("review").expect("close released the lock");
    let events = opened.core.state().unwrap()["events"].clone();
    assert!(!wait_for(|| cwd.join("after-close.txt").exists(), 2500), "old shell outlived its session lock");
    assert_eq!(opened.core.state().unwrap()["events"], events);
}

#[test]
fn review_turn_timeout_must_stop_running_shell() {
    let _env = env_guard("review-shell-timeout");
    if !teamagents_engine::tools::bwrap_available() {
        eprintln!("skipped: bwrap is unavailable, cannot exercise real shell isolation");
        return;
    }
    let cwd = isolated_project("shell-timeout");
    let control = teamagents_engine::tools::shell_run("printf works", &cwd, 5, false, None).unwrap();
    assert_eq!(control, "works", "real sandbox must be working for the probe");
    let api = FakeOpenAi::start(|_, index| {
        if index == 0 {
            (
                200,
                tool_call_response(
                    "shell-1",
                    "shell",
                    json!({
                        "command":"touch started; sleep 2; printf late > after-timeout.txt", "timeout":10
                    }),
                ),
            )
        } else {
            (200, text_response("done"))
        }
    });
    let opened = open_chat_session(&cwd, &api, &["shell"], UserConfig::default());
    let mut spec = opened.core.state().unwrap()["spec"].clone();
    spec["limits"]["turn_active_timeout_s"] = json!(1);
    opened.core.call_in_session("save_spec", json!({"spec":spec})).unwrap();
    opened.runtime.start();
    opened.runtime.user_message("run command", false).unwrap();
    assert!(wait_for(|| cwd.join("started").exists(), 5000));
    assert!(wait_for(|| runs(&opened.core).iter().any(|r| r.status == TurnStatus::Failed), 5000));
    assert!(!cwd.join("after-timeout.txt").exists(), "write has not happened when run becomes FAILED");
    let wrote = wait_for(|| cwd.join("after-timeout.txt").exists(), 4000);
    opened.close();
    eprintln!("run FAILED after 1s; shell wrote after failure={wrote}");
    assert!(!wrote, "timed-out turn left its shell alive to perform further writes");
}

#[test]
#[ignore = "v1 后端随 R29 退役（teamagents exec 现为 v2 无头客户端）；等价覆盖：v2_driver::user_cancel_stops_a_running_job、jobs_runner::a_successful_commands_service_outlives_the_job（A12/D-41）、v2_driver 崩溃恢复组"]
fn review_unknown_external_effect_is_not_replayed_after_crash() {
    let _env = env_guard("review-unknown-external");
    if !teamagents_engine::tools::bwrap_available() {
        eprintln!("skipped: bwrap is unavailable, cannot exercise real shell isolation");
        return;
    }
    let base = isolated_project("unknown-external");
    let api = FakeOpenAi::start(|_, _| {
        (
            200,
            tool_call_response(
                "external-1",
                "shell",
                json!({
                    "command":"printf x >> count.txt; sleep 30; touch late.txt", "timeout":40
                }),
            ),
        )
    });
    let team = worker_files(&base, Some(&api));
    let mut spec: Json = serde_json::from_slice(&std::fs::read(&team).unwrap()).unwrap();
    spec["agents"][0]["tool_bindings"] = json!(["shell"]);
    std::fs::write(&team, spec.to_string()).unwrap();
    let mut worker = ProbeWorker::spawn(&base);
    let project = base.join("project");
    let opened = worker.call("open", json!({"cwd":project,"team":team})).unwrap();
    worker.call("user_message", json!({"text":"run once"})).unwrap();
    assert!(wait_for(|| project.join("count.txt").exists(), 5000), "real sandbox never executed");
    worker.kill();
    let mut worker = ProbeWorker::spawn(&base);
    worker.call("open", json!({"cwd":project,"team":team,"resume":opened["session_id"]})).unwrap();
    let state = worker.call("call", json!({"method":"state", "params":{}})).unwrap();
    assert_eq!(state["runs"][0]["status"], "OUTCOME_UNKNOWN", "{state}");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(api.calls(), 1, "uncertain external effect must not trigger a new model/tool call");
    assert_eq!(std::fs::read_to_string(project.join("count.txt")).unwrap(), "x");
    assert!(!project.join("late.txt").exists());
}

#[test]
fn review_executor_refreshes_workspace_and_revoked_bindings() {
    let _env = env_guard("review-executor-revision");
    let cwd = isolated_project("executor-revision");
    let api = FakeOpenAi::start(|_, index| {
        (
            200,
            match index {
                0 | 2 => tool_call_response(
                    &format!("write-{index}"),
                    "write_file",
                    json!({"path":"result.txt", "content":index.to_string()}),
                ),
                4 => tool_call_response("revoked", "write_file", json!({"path":"revoked.txt", "content":"bad"})),
                _ => text_response("done"),
            },
        )
    });
    let opened = open_chat_session(&cwd, &api, &["files"], UserConfig::default());
    opened.runtime.start();
    opened.runtime.user_message("first", false).unwrap();
    assert!(opened.runtime.settle(5));
    let patch = |id: &str, revision: i64, changes: Json| {
        let receipt = submit(
            &opened.core,
            id,
            "leader",
            "apply_topology_patch",
            json!({
                "base_revision":revision,"operations":[{"op":"update_agent","agent_id":"leader","changes":changes}]
            }),
        );
        assert!(receipt.ok, "{receipt:?}");
        assert_eq!(receipt.result["status"], "APPLIED");
    };
    patch("new-root", 1, json!({"workspace_policy":"isolated"}));
    opened.runtime.user_message("second", false).unwrap();
    assert!(opened.runtime.settle(5));
    let isolated = teamagents_engine::sessions::session_paths("review").base.join("members/leader/work");
    assert_eq!(std::fs::read_to_string(cwd.join("result.txt")).unwrap(), "0");
    assert_eq!(std::fs::read_to_string(isolated.join("result.txt")).unwrap(), "2");
    patch("revoke-files", 2, json!({"tool_bindings":[]}));
    opened.runtime.user_message("third", false).unwrap();
    assert!(opened.runtime.settle(5));
    opened.close();
    assert!(!isolated.join("revoked.txt").exists());
    assert!(api.body(5)["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool"
        && m["tool_call_id"] == "revoked"
        && m["content"].as_str().unwrap_or("").contains("not bound")));
}

#[test]
fn review_completed_checkpoint_restores_reply_without_another_model_call() {
    let _env = env_guard("review-final-checkpoint");
    let agent = agent_json("leader", "leader", &[]);
    let core = core_with_spec("final-checkpoint", json!({"leader_id":"leader", "agents":[agent]}));
    let api = FakeOpenAi::start(|_, _| (200, text_response("original final reply")));
    let run: TurnRun = serde_json::from_value(json!({
        "run_id":"finished", "session_id":"final-checkpoint", "agent_id":"leader",
        "config_revision":1, "topology_revision":1, "context_ref":"ctx:leader:1"
    }))
    .unwrap();
    let view = json!({"inbox_delta":[], "delivery_ids":[]});
    let gateway = ToolGateway::new(
        core.clone(),
        "leader",
        "finished",
        ApprovalGate::new(core.clone(), PermissionPolicy::default()),
        None,
    );
    let first = chat_runner(&core, &agent, profile(&api.base_url(), "openai", json!({}), 0), "/tmp");
    assert_eq!(first.start_or_resume(&run, &view, &gateway, &Json::Null).status, TurnStatus::Completed);
    first.close();
    let restored = chat_runner(&core, &agent, profile(&api.base_url(), "openai", json!({}), 0), "/tmp");
    let restored_gateway = ToolGateway::new(
        core.clone(),
        "leader",
        "finished",
        ApprovalGate::new(core, PermissionPolicy::default()),
        None,
    );
    let reconciled = restored.reconcile(&run, &restored_gateway).expect("durable completed checkpoint");
    assert_eq!(reconciled.status, TurnStatus::Completed);
    assert_eq!(reconciled.reply_text.as_deref(), Some("original final reply"));
    let outcome = restored.start_or_resume(&run, &view, &restored_gateway, &Json::Null);
    assert_eq!(outcome.status, TurnStatus::Completed);
    assert_eq!(outcome.reply_text.as_deref(), Some("original final reply"));
    assert_eq!(api.calls(), 1);
    restored.close();
}

#[test]
fn review_tree_commit_recovers_on_both_sides_of_rename() {
    let _env = env_guard("review-tree-journal");
    let agent = agent_json("leader", "leader", &[]);
    let core = core_with_spec("tree-journal", json!({"leader_id":"leader", "agents":[agent]}));
    let api = FakeOpenAi::start(|_, _| (200, text_response("durable reply")));
    let run: TurnRun = serde_json::from_value(json!({
        "run_id":"journal", "session_id":"tree-journal", "agent_id":"leader",
        "config_revision":1, "topology_revision":1, "context_ref":"ctx:leader:1"
    }))
    .unwrap();
    let view = json!({"inbox_delta":[], "delivery_ids":[]});
    let build = || chat_runner(&core, &agent, profile(&api.base_url(), "openai", json!({}), 0), "/tmp");
    let gateway = || {
        ToolGateway::new(
            core.clone(),
            "leader",
            "journal",
            ApprovalGate::new(core.clone(), PermissionPolicy::default()),
            None,
        )
    };
    let first = build();
    assert_eq!(first.start_or_resume(&run, &view, &gateway(), &Json::Null).status, TurnStatus::Completed);
    first.close();
    let dir = first.history_dir().unwrap();
    let tree_path = dir.join("chat_tree.json");
    let cp_path = dir.join("turns/journal.json");
    let tree: Json = serde_json::from_slice(&std::fs::read(&tree_path).unwrap()).unwrap();
    let mut checkpoint: Json = serde_json::from_slice(&std::fs::read(&cp_path).unwrap()).unwrap();
    checkpoint["tree_pending"] = tree["ctx:leader:1"]["nodes"].clone();
    for renamed in [false, true] {
        for reconcile in [false, true] {
            std::fs::write(&cp_path, checkpoint.to_string()).unwrap();
            std::fs::write(
                &tree_path,
                if renamed {
                    tree.to_string()
                } else {
                    json!({"ctx:leader:1":{"nodes":[], "leaf":null, "rewind_epoch":0}}).to_string()
                },
            )
            .unwrap();
            let restored = build();
            let outcome = if reconcile {
                restored.reconcile(&run, &gateway()).unwrap()
            } else {
                restored.start_or_resume(&run, &view, &gateway(), &Json::Null)
            };
            assert_eq!(outcome.status, TurnStatus::Completed, "renamed={renamed}, reconcile={reconcile}: {outcome:?}");
            assert_eq!(outcome.reply_text.as_deref(), Some("durable reply"));
            let actual: Json = serde_json::from_slice(&std::fs::read(&tree_path).unwrap()).unwrap();
            assert_eq!(actual, tree, "journal replay must not duplicate nodes");
            assert_eq!(api.calls(), 1, "saved final response must not be requested again");
            restored.close();
        }
    }
    // Only a deliberate rewind invalidates the old completed checkpoint.
    let restored = build();
    restored.rewind("ctx:leader:1", None).unwrap();
    assert_eq!(restored.start_or_resume(&run, &view, &gateway(), &Json::Null).status, TurnStatus::Completed);
    assert_eq!(api.calls(), 2);
    restored.close();
}

#[test]
fn review_completed_turns_survive_tree_migration_and_restart() {
    let _env = env_guard("review-history-roundtrip");
    let cwd = isolated_project("history-roundtrip");
    let api = FakeOpenAi::start(|_, index| (200, text_response(&format!("UNIQUE_REPLY_{index}"))));
    let opened = open_chat_session(&cwd, &api, &[], UserConfig::default());
    let dir = teamagents_engine::sessions::session_paths("review").base.join("members/leader");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("chat_history.json"),
        json!({"ctx:leader:1":[
            {"role":"user","content":"legacy input"}, {"role":"assistant","content":"legacy reply"}
        ]})
        .to_string(),
    )
    .unwrap();
    opened.runtime.start();
    for text in ["first", "second"] {
        opened.runtime.user_message(text, false).unwrap();
        assert!(opened.runtime.settle(5));
    }
    opened.close();
    let tree: Json = serde_json::from_slice(&std::fs::read(dir.join("chat_tree.json")).unwrap()).unwrap();
    let replies: Vec<_> = tree["ctx:leader:1"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["message"]["role"] == "assistant")
        .map(|n| n["message"]["content"].as_str().unwrap())
        .collect();
    assert_eq!(replies, vec!["legacy reply", "UNIQUE_REPLY_0", "UNIQUE_REPLY_1"]);
    let resumed = open_chat_session(&cwd, &api, &[], UserConfig::default());
    resumed.runtime.start();
    resumed.runtime.user_message("third", false).unwrap();
    assert!(resumed.runtime.settle(5));
    assert!(api.body(2)["messages"].to_string().contains("UNIQUE_REPLY_1"));
    resumed.close();
}

#[test]
fn review_model_override_preserves_cancellation_and_applies_next_turn() {
    let _env = env_guard("review-model-cancel");
    if !teamagents_engine::tools::bwrap_available() {
        eprintln!("skipped: bwrap is unavailable, cannot exercise real shell isolation");
        return;
    }
    let cwd = isolated_project("model-cancel");
    let api = FakeOpenAi::start(|_, index| {
        (
            200,
            if index == 0 {
                tool_call_response(
                    "shell-1",
                    "shell",
                    json!({"command":"touch started; sleep 2; echo BAD > late", "timeout":10}),
                )
            } else {
                text_response("done")
            },
        )
    });
    let replacement = FakeOpenAi::start(|_, _| (200, json!({"content":[{"type":"text","text":"replacement done"}]})));
    let mut catalog = UserConfig::default();
    catalog.models.insert(
        "replacement".into(),
        ModelProfile {
            provider: "anthropic".into(),
            model: "replacement-model".into(),
            context_window: Some(200000),
            ..profile(&replacement.base_url(), "anthropic", json!({"max_tokens":4321}), 0)
        },
    );
    let opened = open_chat_session(&cwd, &api, &["shell"], catalog);
    opened.runtime.start();
    opened.runtime.user_message("run shell", false).unwrap();
    assert!(wait_for(|| cwd.join("started").exists(), 5000), "real sandbox did not start");
    let run = runs(&opened.core).into_iter().find(|r| r.status == TurnStatus::Running).unwrap();
    opened.set_model_selection("leader", Some("replacement".into()), None, Some("HIGH".into())).unwrap();
    let receipt = submit(&opened.core, "cancel", "user", "cancel_run", json!({"run_id":run.run_id}));
    assert!(receipt.ok);
    assert!(wait_for(
        || runs(&opened.core).iter().any(|r| r.run_id == run.run_id && r.status == TurnStatus::Cancelled),
        5000
    ));
    assert!(!wait_for(|| cwd.join("late").exists(), 2300), "cancelled shell continued writing");
    opened.runtime.user_message("next turn", false).unwrap();
    assert!(opened.runtime.settle(5));
    assert_eq!(api.calls(), 1, "the next turn must use the new provider endpoint");
    assert_eq!(replacement.body(0)["model"], "replacement-model");
    assert_eq!(replacement.body(0)["output_config"]["effort"], "high");
    assert_eq!(replacement.body(0)["max_tokens"], 4321);
    assert!(replacement.body(0).get("reasoning_effort").is_none(), "Anthropic uses output_config.effort");
    assert_eq!(opened.usage_report()["agents"][0]["context_window"], 200000);
    opened.close();
}

#[test]
fn review_late_compaction_cannot_write_after_session_close() {
    let _env = env_guard("review-late-compaction");
    let cwd = isolated_project("late-compaction");
    let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let release = released.clone();
    let api = FakeOpenAi::start(move |body, index| {
        if index == 1 {
            assert!(body["messages"][0]["content"].as_str().unwrap().contains("compacting"));
            assert!(wait_for(|| release.load(Ordering::SeqCst), 5000));
        }
        (200, text_with_usage(if index == 1 { "late summary" } else { "done" }, if index == 0 { 950 } else { 30 }))
    });
    let mut catalog = UserConfig::default();
    let mut prof = profile(&api.base_url(), "openai", json!({}), 0);
    prof.context_window = Some(1000);
    catalog.models.insert("m".into(), prof);
    let opened = open_session(OpenOptions {
        cwd: Some(cwd),
        session_id: Some("late-compact".into()),
        catalog: Some(catalog),
        initial_spec: Some(json!({"leader_id":"leader", "agents":[agent_json("leader", "leader", &[])]})),
        ..Default::default()
    })
    .unwrap();
    opened.runtime.start();
    opened.runtime.user_message("first", false).unwrap();
    assert!(opened.runtime.settle(5));
    opened.runtime.user_message("second", false).unwrap();
    assert!(wait_for(|| api.calls() == 2, 5000));
    opened.close();
    let _lock = teamagents_engine::sessions::acquire_session_lock("late-compact").expect("old ownership released");
    let dir = teamagents_engine::sessions::session_paths("late-compact").base.join("members/leader");
    let before = std::fs::read(dir.join("chat_tree.json")).unwrap();
    released.store(true, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(std::fs::read(dir.join("chat_tree.json")).unwrap(), before);
    assert_eq!(api.calls(), 2);
}

/// D-28 ①+④: over-threshold usage triggers a handoff compaction (summary
/// replaces history, originals stay in the tree) and the model can fetch a
/// covered tool output back with read_history.
#[test]
fn compaction_triggers_on_threshold_and_read_history_recovers_output() {
    let _env = env_guard("chat-compact");
    let server = FakeOpenAi::start(|body, index| match index {
        // first call returns a shell call plus usage way over the 90% of
        // context_window = 10_000 threshold
        0 => (200, tool_call_with_usage("call-1", "shell", shell_args(), 10_000)),
        // the compaction summary call
        1 | 3 => {
            assert!(body["messages"][0]["content"].as_str().unwrap().contains("call-1"));
            (200, text_with_usage("SUMMARY: ran shell, got output", 400))
        }
        // after compaction the model asks for the covered tool output back
        2 | 4 => {
            // Discover the pointer from the request, as a stateless model must.
            let content = body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|m| m["content"].as_str())
                .find(|s| s.contains("Tool output index"))
                .unwrap();
            let id = content
                .lines()
                .find_map(|l| l.strip_prefix("- ").and_then(|l| l.split_once(": shell").map(|(id, _)| id)))
                .unwrap();
            (
                200,
                tool_call_with_usage(
                    &format!("read-{index}"),
                    "read_history",
                    json!({"tool_call_id": id}),
                    if index == 2 { 10_000 } else { 500 },
                ),
            )
        }
        _ => (200, text_with_usage("final answer", 500)),
    });
    let spec = json!({
        "leader_id": "leader",
        "agents": [agent_json("leader", "leader", &["shell"])],
    });
    let core = core_with_spec("s-compact", spec.clone());
    let agent = agent_json("leader", "leader", &["shell"]);
    let mut prof = profile(&server.base_url(), "openai", json!({}), 0);
    prof.context_window = Some(10_000);
    let runner = chat_runner(&core, &agent, prof, "/tmp");
    let (executor, tool_calls) = recording_executor();
    let policy = PermissionPolicy {
        mode: "approved_scope".into(),
        pre_authorized: ["shell"].iter().map(|s| s.to_string()).collect(),
        require_approval: Default::default(),
    };
    let runtime = start_runtime(&core, runner, "leader", policy, RuntimeLimits::default(), executor);

    runtime.user_message("run the shell", false).unwrap();
    assert!(
        wait_for(
            || {
                let rows = runs(&core);
                rows.iter().any(|r| r.status == TurnStatus::Completed) && !rows.iter().any(|r| r.status.is_active())
            },
            15_000
        ),
        "the turn completes through compaction and readback"
    );
    assert_eq!(server.calls(), 6, "tool call -> summary -> read_history -> second summary -> read_history -> final");

    // call 1 is the summary request: the compactor saw the real tool output
    let summary_request = server.body(1)["messages"][0]["content"].as_str().unwrap_or("").to_string();
    assert!(
        summary_request.contains("compacting an agent conversation"),
        "summary prompt, got: {}",
        &summary_request[..summary_request.len().min(120)]
    );
    assert!(summary_request.contains("executed"), "compactor input includes the tool output");

    // call 2 runs on the compacted history: summary present, output gone
    let compacted = server.body(2).to_string();
    assert!(compacted.contains("SUMMARY: ran shell"), "summary carried into the wire history");
    assert!(
        !compacted.contains("\"executed\""),
        "original tool output compacted away: {}",
        &compacted[..compacted.len().min(400)]
    );

    // Even after a second compaction, the original output remains discoverable.
    assert!(server.body(4).to_string().contains("- call-1: shell"));
    let recovered = server.body(5).to_string();
    assert!(recovered.contains("executed"), "read_history returned the original output");

    // shell actually ran once; read_history never reaches the executor
    let called = tool_calls.lock().unwrap().clone();
    assert_eq!(called.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(), vec!["shell"]);
}

#[test]
fn masked_history_readback_keeps_original_page_coordinates_on_the_wire() {
    let _env = env_guard("history-page-pointer");
    let page_args = json!({"tool_call_id":"source-output", "offset":7900, "limit":500});
    let repeated_args = page_args.clone();
    let server = FakeOpenAi::start(move |_, index| {
        let response = match index {
            0 => tool_call_response("source-output", "shell", json!({"command":"source"})),
            1 => tool_call_response("page-first", "read_history", repeated_args.clone()),
            2 => tool_call_response("other-output", "shell", json!({"command":"other"})),
            3 => tool_call_response("small-step", "list_shared", json!({})),
            4 => tool_call_response("page-again", "read_history", repeated_args.clone()),
            _ => text_response("Recovered the original page without wrapping a readback result."),
        };
        (200, response)
    });
    let agent = agent_json("leader", "leader", &["shell"]);
    let core = core_with_spec("history-page-pointer", json!({"leader_id":"leader", "agents":[agent.clone()]}));
    let executions = Arc::new(AtomicUsize::new(0));
    let counter = executions.clone();
    let executor: ToolExecutor = Arc::new(move |_, _, args, _| {
        counter.fetch_add(1, Ordering::SeqCst);
        let output = if args["command"] == "source" {
            format!("{}ORIGINAL_PAGE_MARKER{}", "甲".repeat(8000), "尾".repeat(8000))
        } else {
            "x".repeat(20_000)
        };
        Ok(json!({"output":output}))
    });
    let runtime = start_runtime(
        &core,
        chat_runner(&core, &agent, profile(&server.base_url(), "openai", json!({}), 0), "/tmp"),
        "leader",
        PermissionPolicy::default(),
        RuntimeLimits::default(),
        executor,
    );
    runtime.user_message("Inspect the original page after other tool work.", false).unwrap();
    assert!(runtime.settle(10));
    runtime.close();
    assert!(runs(&core).iter().all(|run| run.status == TurnStatus::Completed));
    assert_eq!(server.calls(), 6);
    let tool_content = |index, id: &str| {
        server.body(index)["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "tool" && m["tool_call_id"] == id)
            .unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let hint = tool_content(4, "page-first");
    for field in [r#""tool_call_id":"source-output""#, r#""offset":7900"#, r#""limit":500"#] {
        assert!(hint.contains(field), "{hint}");
    }
    let first: Json = serde_json::from_str(&tool_content(2, "page-first")).unwrap();
    let recovered: Json = serde_json::from_str(&tool_content(5, "page-again")).unwrap();
    assert_eq!(first, recovered, "rereading must return the same source characters, not another JSON wrapper");
    assert!(recovered["output"].as_str().unwrap().contains("ORIGINAL_PAGE_MARKER"));
    assert_eq!(recovered["offset"], page_args["offset"]);
    assert_eq!(recovered["next_offset"], 8400);
    assert_eq!(executions.load(Ordering::SeqCst), 2, "readback never re-executes either source operation");
}

#[test]
fn large_window_keeps_recent_source_and_history_pages_on_the_wire() {
    for window in [None, Some(64_000), Some(1_000_000)] {
        check_recent_source_visibility(window, false);
    }
}

#[test]
fn private_subagent_large_window_keeps_its_recent_source_and_history_pages() {
    for window in [None, Some(64_000), Some(1_000_000)] {
        check_recent_source_visibility(window, true);
    }
}

fn check_recent_source_visibility(window: Option<u64>, nested: bool) {
    let session = format!("recent-source-{}-{nested}", window.unwrap_or(0));
    let _env = env_guard(&session);
    let server = FakeOpenAi::start(move |body, _| {
        let tool_count = body["messages"].as_array().unwrap().iter().filter(|m| m["role"] == "tool").count();
        let response = if nested && !private_helper_request(body) {
            if tool_count == 0 {
                tool_call_response("helper", "run_subagent", json!({"task":"Compare the two source files."}))
            } else {
                text_response("Received the private comparison.")
            }
        } else {
            match tool_count {
                0 => tool_call_response("first-source", "read_file", json!({"path":"first.rs"})),
                1 => tool_call_response(
                    "first-page",
                    "read_history",
                    json!({"tool_call_id":"first-source", "offset":0, "limit":12_000}),
                ),
                2 => tool_call_response("second-source", "read_file", json!({"path":"second.rs"})),
                3 => tool_call_response(
                    "second-page",
                    "read_history",
                    json!({"tool_call_id":"second-source", "offset":0, "limit":12_000}),
                ),
                _ => text_response("Compared the source files."),
            }
        };
        (200, response)
    });
    let agent = agent_json("leader", "leader", &["files"]);
    let core = core_with_spec(&session, json!({"leader_id":"leader", "agents":[agent.clone()]}));
    let mut prof = profile(&server.base_url(), "openai", json!({}), 0);
    prof.context_window = window;
    let executions = Arc::new(AtomicUsize::new(0));
    let counter = executions.clone();
    let executor: ToolExecutor = Arc::new(move |_, tool, args, _| {
        assert_eq!(tool, "read_file");
        counter.fetch_add(1, Ordering::SeqCst);
        let marker = if args["path"] == "first.rs" { "FIRST_SOURCE" } else { "SECOND_SOURCE" };
        Ok(json!({"output":format!("{marker}\n{}", "x".repeat(20_000))}))
    });
    let runtime = start_runtime(
        &core,
        chat_runner(&core, &agent, prof, "/tmp"),
        "leader",
        PermissionPolicy::default(),
        RuntimeLimits::default(),
        executor,
    );
    runtime.user_message("Compare the source files and their earlier pages.", false).unwrap();
    assert!(runtime.settle(10));
    runtime.close();
    assert!(runs(&core).iter().all(|run| run.status == TurnStatus::Completed));
    assert_eq!(server.calls(), if nested { 7 } else { 5 });
    assert_eq!(executions.load(Ordering::SeqCst), 2, "history pages must not execute the source operation again");

    let request = server.body(if nested { 5 } else { 4 });
    let messages = request["messages"].as_array().unwrap();
    let content = |id: &str| {
        messages.iter().find(|m| m["role"] == "tool" && m["tool_call_id"] == id).unwrap()["content"].as_str().unwrap()
    };
    let keep_older = window == Some(1_000_000);
    for (id, marker) in
        [("first-source", "FIRST_SOURCE"), ("first-page", "FIRST_SOURCE"), ("second-source", "SECOND_SOURCE")]
    {
        assert_eq!(content(id).contains(marker), keep_older, "{session}: recent {id} visibility");
        assert_eq!(content(id).contains("tool output hidden"), !keep_older);
    }
    assert!(content("second-page").contains("SECOND_SOURCE"), "the latest answer remains visible at every window");
    if nested {
        let parent = server.body(6).to_string();
        assert!(!parent.contains("FIRST_SOURCE") && !parent.contains("SECOND_SOURCE"), "helper history stays private");
    }
}

#[test]
fn compaction_after_empty_rewind_keeps_the_new_branch_root_across_restart() {
    let _env = env_guard("compaction-rewound-root");
    let server = FakeOpenAi::start(|_, index| {
        (
            200,
            match index {
                0 => text_with_usage("Finished the old branch.", 100),
                1 => tool_call_with_usage("new-branch-tool", "shell", shell_args(), 10_000),
                2 => text_with_usage("Summary of the new branch only.", 100),
                _ => text_with_usage("Continued the new branch.", 100),
            },
        )
    });
    let agent = agent_json("leader", "leader", &["shell"]);
    let core = core_with_spec("compaction-rewound-root", json!({"leader_id":"leader","agents":[agent.clone()]}));
    let mut prof = profile(&server.base_url(), "openai", json!({}), 0);
    prof.context_window = Some(10_000); // Local fixture, not a real model window.
    let runner = chat_runner(&core, &agent, prof.clone(), "/tmp");
    let history_path = runner.history_dir().unwrap().join("chat_tree.json");
    let runtime = start_runtime(
        &core,
        runner.clone(),
        "leader",
        PermissionPolicy::default(),
        RuntimeLimits::default(),
        Arc::new(|_, _, _, _| Ok(json!({"output":"new branch output"}))),
    );
    runtime.user_message("OLD_BRANCH_ONLY", false).unwrap();
    assert!(runtime.settle(5));
    let original: Json = serde_json::from_slice(&std::fs::read(&history_path).unwrap()).unwrap();
    runner.rewind("ctx:leader:1", None).unwrap();
    runtime.user_message("NEW_BRANCH_ONLY", false).unwrap();
    assert!(runtime.settle(5));
    runtime.close();
    assert!(runs(&core).iter().all(|run| run.status == TurnStatus::Completed));
    assert_eq!(server.calls(), 4);
    let saved: Json = serde_json::from_slice(&std::fs::read(&history_path).unwrap()).unwrap();
    let nodes = saved["ctx:leader:1"]["nodes"].as_array().unwrap();
    let old_nodes = original["ctx:leader:1"]["nodes"].as_array().unwrap();
    assert!(nodes.starts_with(old_nodes), "rewind and compaction preserve the abandoned branch");
    let new_root = nodes.iter().find(|node| node["parent"].is_null() && node["id"] != old_nodes[0]["id"]).unwrap();
    let summary = nodes.iter().find(|node| node["skip_to"].is_string()).unwrap();
    assert_eq!(summary["skip_to"], new_root["id"], "summary must not jump to an abandoned root");
    for index in 1..4 {
        assert!(!server.body(index).to_string().contains("OLD_BRANCH_ONLY"));
    }
    let restarted = start_runtime(
        &core,
        chat_runner(&core, &agent, prof, "/tmp"),
        "leader",
        PermissionPolicy::default(),
        RuntimeLimits::default(),
        Arc::new(|_, _, _, _| panic!("no new tool execution expected")),
    );
    restarted.user_message("Resume the current branch.", false).unwrap();
    assert!(restarted.settle(5));
    restarted.close();
    assert!(runs(&core).iter().all(|run| run.status == TurnStatus::Completed));
    assert_eq!(server.calls(), 5);
    assert!(!server.body(4).to_string().contains("OLD_BRANCH_ONLY"));
    assert!(server.body(4).to_string().contains("NEW_BRANCH_ONLY"));
}

#[test]
fn long_context_compaction_preserves_request_and_reads_large_output_after_restart() {
    let _env = env_guard("long-context-request");
    const REQUEST: &str = "请修复解析器；必须保留 CRLF、空字段和用户已有改动。";
    let server = FakeOpenAi::start(|_, index| match index {
        0 => (200, tool_call_with_usage("large-output", "shell", shell_args(), 10_000)),
        1 | 3 => (200, text_with_usage("Summary deliberately omits the user's exact constraints.", 100)),
        2 => (
            200,
            tool_call_with_usage(
                "read-first",
                "read_history",
                json!({"tool_call_id":"large-output", "offset":29_900, "limit":500}),
                10_000,
            ),
        ),
        4 => (200, text_with_usage("first turn complete", 100)),
        5 => (
            200,
            tool_call_with_usage(
                "read-restarted",
                "read_history",
                json!({"tool_call_id":"large-output", "offset":29_900, "limit":500}),
                100,
            ),
        ),
        _ => (200, text_with_usage("recovered the original output", 100)),
    });
    let agent = agent_json("leader", "leader", &["shell"]);
    let core = core_with_spec("long-context-request", json!({"leader_id":"leader", "agents":[agent.clone()]}));
    let mut prof = profile(&server.base_url(), "openai", json!({}), 0);
    prof.context_window = Some(10_000);
    let executions = Arc::new(AtomicUsize::new(0));
    let counter = executions.clone();
    let executor: ToolExecutor = Arc::new(move |_, _, _, _| {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"output":format!("{}ORIGINAL-MIDDLE-MARKER{}", "H".repeat(30_000), "T".repeat(30_000))}))
    });
    let first = start_runtime(
        &core,
        chat_runner(&core, &agent, prof.clone(), "/tmp"),
        "leader",
        PermissionPolicy::default(),
        RuntimeLimits::default(),
        executor.clone(),
    );
    first.user_message(REQUEST, false).unwrap();
    assert!(wait_for(|| runs(&core).iter().any(|r| r.status == TurnStatus::Completed), 5_000));
    first.close();
    assert_eq!(server.calls(), 5);
    for index in [2, 4] {
        let body = server.body(index).to_string();
        assert!(body.contains(REQUEST), "compaction {index} lost the latest user request");
        assert!(
            !body.contains(&"H".repeat(10_000)),
            "a covered large output must not immediately overflow the compacted request"
        );
        assert_eq!(
            body.matches("- large-output: shell").count(),
            1,
            "retained tool groups must not duplicate the output index"
        );
    }
    assert!(server.body(3).to_string().contains("ORIGINAL-MIDDLE-MARKER"));
    let second = start_runtime(
        &core,
        chat_runner(&core, &agent, prof, "/tmp"),
        "leader",
        PermissionPolicy::default(),
        RuntimeLimits::default(),
        executor,
    );
    second.user_message("继续核对刚才的完整输出", false).unwrap();
    assert!(wait_for(|| runs(&core).iter().filter(|r| r.status == TurnStatus::Completed).count() == 2, 5_000));
    second.close();
    assert_eq!(server.calls(), 7);
    assert!(server.body(6).to_string().contains("ORIGINAL-MIDDLE-MARKER"));
    assert_eq!(executions.load(Ordering::SeqCst), 1, "restart must read the checkpoint instead of replaying shell");
}

#[test]
fn long_context_keeps_small_recent_groups_when_the_latest_reasoning_is_oversized() {
    let _env = env_guard("long-context-recent-groups");
    let server = FakeOpenAi::start(|_, index| match index {
        0 => (200, tool_call_with_usage("source-read", "shell", shell_args(), 100)),
        1 => {
            let mut reply = tool_call_with_usage("large-reasoning", "list_shared", json!({}), 10_000);
            reply["choices"][0]["message"]["reasoning_content"] = json!("R".repeat(20_000));
            (200, reply)
        }
        2 => (200, text_with_usage("Inspected the files; implement the fix next.", 100)),
        _ => (200, text_with_usage("done", 100)),
    });
    let agent = agent_json("leader", "leader", &["shell"]);
    let core = core_with_spec("long-context-recent-groups", json!({"leader_id":"leader", "agents":[agent.clone()]}));
    let mut prof = profile(&server.base_url(), "openai", json!({}), 0);
    prof.context_window = Some(10_000);
    let executions = Arc::new(AtomicUsize::new(0));
    let counter = executions.clone();
    let executor: ToolExecutor = Arc::new(move |_, _, _, _| {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"output":"SOURCE-NEEDED-FOR-THE-NEXT-EDIT"}))
    });
    let runtime = start_runtime(
        &core,
        chat_runner(&core, &agent, prof, "/tmp"),
        "leader",
        PermissionPolicy::default(),
        RuntimeLimits::default(),
        executor,
    );
    runtime.user_message("修复代码并保留用户文件", false).unwrap();
    assert!(wait_for(|| runs(&core).iter().any(|r| r.status == TurnStatus::Completed), 5_000));
    runtime.close();
    assert_eq!(server.calls(), 4);
    let next = server.body(3).to_string();
    assert!(next.contains("修复代码并保留用户文件"));
    assert!(
        next.contains("SOURCE-NEEDED-FOR-THE-NEXT-EDIT"),
        "a large last group must not discard earlier small source reads"
    );
    assert!(!next.contains(&"R".repeat(10_000)));
    assert_eq!(executions.load(Ordering::SeqCst), 1);
}

#[test]
fn long_context_summary_calls_consume_the_persisted_model_step_budget() {
    let _env = env_guard("long-context-budget");
    let server = FakeOpenAi::start(|body, _| {
        if body["messages"][0]["content"].as_str().unwrap_or("").contains("compacting an agent conversation") {
            (200, text_with_usage("Summary of the completed tool call.", 100))
        } else {
            (200, tool_call_with_usage("budget-call", "list_shared", json!({}), 10_000))
        }
    });
    let agent = agent_json("leader", "leader", &[]);
    let core = core_with_spec(
        "long-context-budget",
        json!({"leader_id":"leader", "agents":[agent.clone()], "limits":{"max_model_steps_per_turn":2}}),
    );
    let mut prof = profile(&server.base_url(), "openai", json!({}), 0);
    prof.context_window = Some(10_000);
    let runner = chat_runner(&core, &agent, prof.clone(), "/tmp");
    let history_dir = runner.history_dir().unwrap();
    let (executor, _) = recording_executor();
    let runtime =
        start_runtime(&core, runner, "leader", PermissionPolicy::default(), RuntimeLimits::default(), executor);
    runtime.user_message("keep inspecting", false).unwrap();
    assert!(wait_for(|| runs(&core).iter().any(|r| r.status == TurnStatus::Failed), 5_000));
    runtime.close();
    assert_eq!(server.calls(), 2, "the summary request must count toward the two-call limit");
    assert!(event_kinds(&core).contains(&"limit_reached".into()));
    let run = runs(&core).remove(0);
    let checkpoint: Json =
        serde_json::from_slice(&std::fs::read(history_dir.join("turns").join(format!("{}.json", run.run_id))).unwrap())
            .unwrap();
    assert_eq!(checkpoint["model_steps"], 2);
    let restored = chat_runner(&core, &agent, prof, "/tmp");
    let gateway = ToolGateway::new(
        core.clone(),
        "leader",
        &run.run_id,
        ApprovalGate::new(core.clone(), PermissionPolicy::default()),
        None,
    );
    let outcome = restored.start_or_resume(&run, &json!({}), &gateway, &Json::Null);
    assert_eq!(outcome.status, TurnStatus::Failed);
    assert_eq!(server.calls(), 2, "a restart must preserve the exhausted budget");
}

#[test]
fn long_context_overflow_cannot_start_recovery_after_budget_exhaustion() {
    let _env = env_guard("long-context-overflow-budget");
    let server = FakeOpenAi::start(|_, index| {
        if index == 0 {
            (400, json!({"error":{"code":"context_length_exceeded"}}))
        } else {
            (200, text_response("summary after the limit"))
        }
    });
    let agent = agent_json("leader", "leader", &[]);
    let core = core_with_spec(
        "long-context-overflow-budget",
        json!({"leader_id":"leader", "agents":[agent.clone()], "limits":{"max_model_steps_per_turn":1}}),
    );
    let runner = chat_runner(&core, &agent, profile(&server.base_url(), "openai", json!({}), 0), "/tmp");
    let (executor, _) = recording_executor();
    let runtime =
        start_runtime(&core, runner, "leader", PermissionPolicy::default(), RuntimeLimits::default(), executor);
    runtime.user_message("start work", false).unwrap();
    assert!(wait_for(|| runs(&core).iter().any(|r| r.status == TurnStatus::Failed), 5_000));
    runtime.close();
    assert_eq!(server.calls(), 1, "overflow recovery must not make an unbudgeted summary call");
    assert!(event_kinds(&core).contains(&"limit_reached".into()));
}

#[test]
fn long_context_provider_overflow_recovers_without_repeating_the_large_tool_output() {
    let _env = env_guard("long-context-overflow-recovery");
    let server = FakeOpenAi::start(|body, index| {
        if index == 0 {
            (200, tool_call_response("overflow-output", "shell", shell_args()))
        } else if body["messages"][0]["content"].as_str().unwrap_or("").contains("compacting an agent conversation") {
            (200, text_response("The tool ran successfully. Continue the requested code review."))
        } else if body.to_string().contains(&"H".repeat(10_000)) {
            (400, json!({"error":{"code":"context_length_exceeded"}}))
        } else {
            (200, text_response("recovered successfully"))
        }
    });
    let agent = agent_json("leader", "leader", &["shell"]);
    let core = core_with_spec(
        "long-context-overflow-recovery",
        json!({"leader_id":"leader", "agents":[agent.clone()], "limits":{"max_model_steps_per_turn":4}}),
    );
    let runner = chat_runner(&core, &agent, profile(&server.base_url(), "openai", json!({}), 0), "/tmp");
    let executor: ToolExecutor = Arc::new(|_, _, _, _| Ok(json!({"output":"H".repeat(60_000)})));
    let runtime =
        start_runtime(&core, runner, "leader", PermissionPolicy::default(), RuntimeLimits::default(), executor);
    runtime.user_message("Review the parser without changing the public API.", false).unwrap();
    assert!(wait_for(|| runs(&core).iter().any(|r| r.status.is_terminal()), 5_000));
    runtime.close();
    assert_eq!(runs(&core)[0].status, TurnStatus::Completed);
    assert_eq!(server.calls(), 4, "tool request, overflow, summary and one successful recovery");
    assert!(server.body(3).to_string().contains("without changing the public API"));
    assert!(server.body(3).to_string().contains("- overflow-output: shell"));
}
