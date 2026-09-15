//! End-to-end chat member tests: a fake OpenAI HTTP service + the real
//! ChatRunner driven through the real core and runtime (review F-6).
//!
//! Covers: once-approval consumption and EXPIRED handling (F-3/F-12), denial,
//! the model-step budget (F-1), active-timeout interruption (F-2/F-9),
//! conversation persistence across a restart (F-4), HTTP retry policy (F-11)
//! and the reasoning-effort fallback (F-7).

mod support;

use serde_json::{json, Value as Json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use support::{core_with_spec, isolated_state_home, submit, wait_for};
use teamagents_core::models::{ModelProfile, TurnRun, TurnStatus, UserConfig};
use teamagents_engine::bound::BoundTools;
use teamagents_engine::chat::ChatRunner;
use teamagents_engine::core_client::CoreClient;
use teamagents_engine::gateway::{ApprovalGate, PermissionPolicy, ToolGateway};
use teamagents_engine::runtime::{AgentRunner, Notify, Runtime, RuntimeLimits, ToolExecutor};

/// Tests in this binary share process env (XDG_STATE_HOME); serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_guard(tag: &str) -> MutexGuard<'static, ()> {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    isolated_state_home(tag);
    guard
}

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
    chat_runner_with(core, agent, profile, workdir, Notify::new(core.clone()))
}

fn chat_runner_with(core: &Arc<CoreClient>, agent: &Json, profile: ModelProfile, workdir: &str, notify: Arc<Notify>) -> Arc<ChatRunner> {
    let bindings: Vec<String> = agent
        .get("tool_bindings")
        .and_then(|v| v.as_array())
        .map(|items| items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let bound = BoundTools::load(&UserConfig::default(), &bindings).expect("bound tools");
    ChatRunner::new(
        agent,
        profile,
        Some(workdir.to_string()),
        notify,
        bound,
        vec![],
        (false, false),
    )
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

fn recording_executor() -> (ToolExecutor, Arc<Mutex<Vec<(String, Json)>>>) {
    let calls: Arc<Mutex<Vec<(String, Json)>>> = Arc::new(Mutex::new(vec![]));
    let sink = calls.clone();
    let executor: ToolExecutor = Arc::new(move |_agent: &str, tool: &str, args: &Json, _control: &teamagents_engine::gateway::TurnControl| {
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
        .and_then(|reply| reply.get("approval").and_then(|a| a.get("status")).and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_default()
}

fn shell_args() -> Json {
    json!({"command": "echo hi"})
}

/// A member whose turn thread panics (F-9).
struct PanicRunner;

impl AgentRunner for PanicRunner {
    fn start_or_resume(&self, _run: &TurnRun, _view: &Json, _gateway: &ToolGateway, _wake: &Json) -> teamagents_core::control::TurnOutcome {
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

/// F-3: a once approval must be found again when the model re-sends the call
/// with a fresh tool_call_id, the operation must run, and the row must be
/// consumed (EXPIRED) — then the turn finishes.
/// The Responses wire format (`/responses`) must round-trip tool calls: the
/// engine keeps chat-completions history internally, so this checks both the
/// translation out (`function_call_output`) and back (`tool_calls`).
#[test]
fn responses_protocol_round_trips_a_tool_call() {
    let _env = env_guard("chat-responses");
    let sse = |frames: Vec<Json>| -> String {
        frames.iter().map(|frame| format!("event: x\ndata: {frame}\n\n")).collect()
    };
    let server = FakeOpenAi::start_sse(move |_body, index| {
        if index == 0 {
            sse(vec![
                json!({"type":"response.output_item.done","item":{"type":"function_call","call_id":"call-1","name":"shell","arguments":"{\"command\":\"echo hi\"}"}}),
                json!({"type":"response.completed","response":{"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15},
                    "output":[{"type":"function_call","call_id":"call-1","name":"shell","arguments":"{\"command\":\"echo hi\"}"}]}}),
            ])
        } else {
            sse(vec![json!({"type":"response.completed","response":{"usage":{"input_tokens":12,"output_tokens":3,"total_tokens":15},
                "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"all done"}]}]}})])
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
    let runtime = start_runtime(&core, runner, "leader", PermissionPolicy::default(), RuntimeLimits::default(), executor);

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
    assert!(second["instructions"].as_str().unwrap_or("").contains("Leader"), "system prompt maps to instructions: {second}");
    let shell_tool = second["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "shell")
        .cloned()
        .unwrap_or(Json::Null);
    assert_eq!(shell_tool["type"], "function", "tools are flattened for Responses: {shell_tool}");
    assert!(shell_tool.get("function").is_none(), "no nested function object: {shell_tool}");
    assert_eq!(shell_tool["parameters"]["properties"]["command"]["type"], "string");
    assert!(second["stream"].as_bool().unwrap_or(false), "streaming stays on: {second}");
    runtime.close();
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
    std::fs::write(&script, format!("#!/bin/sh\nprintf '%s\\n' \"$1\" >> {}\ncat >> {}\n", log.display(), log.display())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let api = FakeOpenAi::start(|_, index| {
        (200, if index == 0 {
            tool_call_response("call-1", "write_file", json!({"path": "a.txt", "content": "x"}))
        } else {
            text_response("done")
        })
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
    let dir = std::env::temp_dir().join(format!("ta-view-image-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
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
    let runner = chat_runner(&core, &agent, profile(&server.base_url(), "openai", json!({}), 0), &dir.to_string_lossy());
    // the real workspace executor, so the tool actually reads the file
    let workspace = teamagents_engine::tools::workspace_executor(dir.clone(), None);
    let executor: ToolExecutor = Arc::new(move |_agent: &str, tool: &str, args: &Json, _control: &teamagents_engine::gateway::TurnControl| workspace(tool, args));
    let runtime = start_runtime(&core, runner, "leader", PermissionPolicy::default(), RuntimeLimits::default(), executor);

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
    let runner = chat_runner_with(&core, &agent, profile(&server.base_url(), "openai", json!({}), 0), "/tmp", notify.clone());
    let (executor, _tool_calls) = recording_executor();
    let runtime = start_runtime_with(&core, runner, "leader", approval_policy_for_shell(), RuntimeLimits::default(), executor, notify.clone());

    let seen: Arc<Mutex<Vec<Json>>> = Arc::new(Mutex::new(vec![]));
    let sink = seen.clone();
    notify.set_tool_sink(Box::new(move |run_id, agent_id, activity| {
        sink.lock().unwrap().push(json!({"run_id": run_id, "agent_id": agent_id, "activity": activity.clone()}));
    }));

    runtime.user_message("write the file", false).unwrap();
    assert!(wait_for(|| !seen.lock().unwrap().is_empty(), 15_000), "tool activity must be reported while the turn runs");
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
    let runtime = start_runtime(&core, runner, "leader", approval_policy_for_shell(), RuntimeLimits::default(), executor);

    runtime.user_message("run the shell", false).unwrap();
    assert!(
        wait_for(|| !pending_approvals(&core).is_empty(), 10_000),
        "the shell call parks for approval"
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
            15_000
        ),
        "the turn completes after the once approval"
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
    let runtime = start_runtime(&core, runner, "leader", approval_policy_for_shell(), RuntimeLimits::default(), executor);

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
    let runtime = start_runtime(&core, runner, "leader", approval_policy_for_shell(), RuntimeLimits::default(), executor);

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
    let runtime = start_runtime(&core, runner, "leader", PermissionPolicy::default(), RuntimeLimits::default(), executor);

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
        .and_then(|events| {
            events
                .iter()
                .rev()
                .find(|e| e.get("kind").and_then(|v| v.as_str()) == Some("run_failed"))
        })
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
        (200, tool_call_response(&format!("call-{index}"), "publish_shared",
                                 json!({"space_id": "main", "content": format!("tick-{index}")})))
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
    let runtime = start_runtime(&core, runner, "leader", PermissionPolicy::default(), RuntimeLimits::default(), executor);

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
    let core = core_with_spec(
        "s-panic",
        json!({"leader_id": "leader", "agents": [agent_json("leader", "leader", &[])]}),
    );
    let notify = Notify::new(core.clone());
    let approvals = ApprovalGate::new(core.clone(), PermissionPolicy::default());
    let executor: ToolExecutor = Arc::new(|_agent: &str, tool: &str, _args: &Json, _control: &teamagents_engine::gateway::TurnControl| Err(format!("no executor for {tool}")));
    let runtime = Runtime::new(core.clone(), notify, approvals, executor, None, RuntimeLimits::default());
    runtime.add_runner("leader", Arc::new(PanicRunner));
    runtime.start();

    runtime.user_message("trigger the crash", false).unwrap();
    assert!(
        wait_for(|| {
            let rows = runs(&core);
            !rows.is_empty() && !rows.iter().any(|r| r.status.is_active())
        }, 10_000),
        "the crashed turn is finalised"
    );
    let error = core
        .state()
        .unwrap()
        .get("events")
        .and_then(|v| v.as_array())
        .and_then(|events| {
            events
                .iter()
                .rev()
                .find(|e| e.get("kind").and_then(|v| v.as_str()) == Some("run_failed"))
        })
        .and_then(|e| e.pointer("/payload/error").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_default();
    assert!(error.contains("member runner crashed"), "unexpected error {error:?}");
    assert!(
        !error.contains("active-time limit"),
        "a crash must not be reported as a timeout: {error:?}"
    );
    runtime.close();
}

/// F-4: the member's conversation history is reloaded after a restart.
#[test]
fn conversation_history_survives_a_restart() {
    let _env = env_guard("chat-history");
    let server = FakeOpenAi::start(|_body, _index| (200, text_response("first-reply")));
    let core = core_with_spec(
        "s-history",
        json!({"leader_id": "leader", "agents": [agent_json("leader", "leader", &[])]}),
    );
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
    let core = core_with_spec(
        "s-retry-401",
        json!({"leader_id": "leader", "agents": [agent_json("leader", "leader", &[])]}),
    );
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
    let core = core_with_spec(
        "s-retry-500",
        json!({"leader_id": "leader", "agents": [agent_json("leader", "leader", &[])]}),
    );
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
    let core = core_with_spec(
        "s-effort-ds",
        json!({"leader_id": "leader", "agents": [agent_json("leader", "leader", &[])]}),
    );
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
    let outcome = runner.start_or_resume(&run("run-effort-ds", "s-effort-ds"), &view, &gateway, &json!({"reason": "new_input"}));
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
    let core = core_with_spec(
        "s-effort-oa",
        json!({"leader_id": "leader", "agents": [agent_json("leader", "leader", &[])]}),
    );
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
    let outcome = runner.start_or_resume(&run("run-effort-oa", "s-effort-oa"), &view, &gateway, &json!({"reason": "new_input"}));
    assert_eq!(outcome.status, TurnStatus::Completed);
    assert_eq!(rejected.calls(), 2, "one rejection plus one fallback retry");
    assert_eq!(rejected.body(0)["reasoning_effort"], json!("xhigh"));
    assert_eq!(rejected.body(1)["reasoning_effort"], json!("max"));
}

// Follow-up review regressions: real member, process and sandbox boundaries.
use teamagents_engine::session::{open_session, OpenOptions};
use std::time::Duration;

fn isolated_project(tag: &str) -> std::path::PathBuf {
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("ta-review-{tag}-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
    dir
}

fn open_chat_session(cwd: &std::path::Path, api: &FakeOpenAi, bindings: &[&str], mut catalog: UserConfig) -> Arc<teamagents_engine::session::OpenedSession> {
    catalog.models.insert("m".into(), profile(&api.base_url(), "openai", json!({}), 0));
    catalog.models.insert("other".into(), ModelProfile { model: "new-model".into(), ..profile(&api.base_url(), "openai", json!({}), 0) });
    open_session(OpenOptions {
        cwd: Some(cwd.to_path_buf()), session_id: Some("review".into()),
        initial_spec: Some(json!({"leader_id":"leader", "agents":[agent_json("leader", "leader", bindings)]})),
        catalog: Some(catalog), ..Default::default()
    }).unwrap()
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
    let receipt = submit(&opened.core, "update-model", "leader", "apply_topology_patch", json!({
        "base_revision":1, "operations":[{"op":"update_agent", "agent_id":"leader",
        "changes":{"model_profile":"other", "instructions":"NEW INSTRUCTIONS", "tool_bindings":[]}}]
    }));
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
        (200, if index == 0 {
            tool_call_response("topo-1", "apply_topology_patch", json!({
                "base_revision": 1,
                "operations": [
                    // no tool_bindings: the member must inherit the Leader's
                    {"op":"add_agent","agent":{"id":"w1","name":"W1","role":"worker","runtime_kind":"deepagents"}},
                    // explicit [] stays messaging-only
                    {"op":"add_agent","agent":{"id":"w2","name":"W2","role":"worker","runtime_kind":"deepagents","tool_bindings":[]}},
                ]
            }))
        } else {
            text_response("done")
        })
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
        (200, if index == 0 {
            tool_call_response("topo-1", "apply_topology_patch", json!({
                "base_revision": 1,
                "operations": [
                    {"op":"add_agent","agent":{"id":"w1","name":"W1","role":"worker",
                        "runtime_kind":"deepagents","tool_bindings":[]}},
                    {"op":"add_agent","agent":{"id":"w2","name":"W2","role":"worker",
                        "runtime_kind":"deepagents","model_profile":"gpt-9","tool_bindings":[]}},
                ]
            }))
        } else {
            text_response("done")
        })
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
fn review_bound_mcp_must_be_advertised_to_model() {
    let _env = env_guard("review-mcp");
    let cwd = isolated_project("mcp");
    let api = FakeOpenAi::start(|_, index| {
        (200, if index == 0 { tool_call_response("mcp-1", "echo_echo", json!({"text":"ping"})) }
        else { text_response("done") })
    });
    let mut catalog = UserConfig::default();
    catalog.tools.insert("echo_service".into(), serde_json::from_value(json!({
        "kind":"mcp", "mcp_server":"echo", "mcp_execution":"host", "command":env!("CARGO_BIN_EXE_fake-mcp-server"),
        "tool_names":["echo"], "required":true
    })).unwrap());
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
    assert!(api.body(1)["messages"].as_array().unwrap().iter()
        .any(|m| m["role"] == "tool" && m["content"].as_str().unwrap_or("").contains("ping")));
    let names: Vec<&str> = body["tools"].as_array().unwrap().iter().filter_map(|t| t["function"]["name"].as_str()).collect();
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
        } else { (200, text_response("done")) }
    });
    let opened = open_chat_session(&cwd, &api, &["files"], UserConfig::default());
    opened.runtime.start();
    opened.runtime.user_message("wait", false).unwrap();
    assert!(wait_for(|| api.calls() == 1, 5000));
    opened.close();
    let lock = teamagents_engine::sessions::acquire_session_lock("review").expect("close released the lock");
    let events_after_close = opened.core.state().unwrap()["events"].clone();
    let wrote = wait_for(|| cwd.join("after-close.txt").exists(), 2000);
    assert_eq!(opened.core.state().unwrap()["events"], events_after_close, "old runtime finalized after releasing ownership");
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
            .arg("serve").env("XDG_CONFIG_HOME", base.join("config")).env("XDG_STATE_HOME", base.join("state"))
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, replies) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
                let message: Json = serde_json::from_str(&line).unwrap();
                if message.get("id").is_some() { let _ = tx.send(message); }
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
        if let Some(err) = r.get("error") { Err(err.to_string()) } else { Ok(r["result"].clone()) }
    }
    fn entries(&mut self) -> Json {
        self.call("call", json!({"method":"shared_entries", "params":{"space_ids":["main"]}})).unwrap()["entries"].clone()
    }
    fn kill(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); }
}
impl Drop for ProbeWorker { fn drop(&mut self) { self.kill(); } }

fn worker_files(base: &std::path::Path, api: Option<&FakeOpenAi>) -> std::path::PathBuf {
    std::fs::create_dir_all(base.join("config/teamagents")).unwrap();
    let mut config = "[models.m]\nprovider='openai'\nprotocol='openai'\nmodel='test'\nmax_retries=0\n".to_string();
    if let Some(api) = api { config.push_str(&format!("base_url='{}'\n", api.base_url())); }
    std::fs::write(base.join("config/teamagents/config.toml"), config).unwrap();
    let team = base.join("team.json");
    std::fs::write(&team, json!({"leader_id":"leader", "agents":[agent_json("leader", "leader", &["files"])],
        "shared_spaces":[{"id":"main", "readers":["leader"], "writers":["leader"]}]}).to_string()).unwrap();
    team
}

#[test]
fn review_crash_after_committed_chat_action_must_not_replay_it() {
    let _env = env_guard("review-recovery");
    for lost_receipt in [false, true] {
    let base = isolated_project("recovery");
    let api = FakeOpenAi::start(|body, index| {
        // The model must see the committed result after restart. An API that
        // deliberately requests the same operation again is a different task.
        if index == 1 { std::thread::sleep(Duration::from_millis(500)); }
        let has_result = body["messages"].as_array().unwrap().iter().any(|m|
            m["role"] == "tool" && m["content"].as_str().unwrap_or("").contains("sequence"));
        (200, if has_result { text_response("done") } else {
            tool_call_response(&format!("publish-{index}"), "publish_shared", json!({"space_id":"main", "content":"once-only"}))
        })
    });
    let team = worker_files(&base, Some(&api));
    let mut worker = ProbeWorker::spawn(&base);
    let opened = worker.call("open", json!({"cwd":base,"team":team})).unwrap();
    worker.call("user_message", json!({"text":"publish once"})).unwrap();
    assert!(wait_for(|| api.calls() >= 2, 5000));
    assert_eq!(worker.entries().as_array().unwrap().len(), 1);
    let state = worker.call("call", json!({"method":"state", "params":{}})).unwrap();
    let run_id = state["runs"][0]["run_id"].as_str().unwrap();
    worker.kill();
    if lost_receipt {
        // Inject exactly the commit -> receipt-checkpoint crash window, using
        // the real model ID and already committed SQLite action receipt.
        let path = base.join("state/teamagents/sessions").join(opened["session_id"].as_str().unwrap())
            .join("members/leader/turns").join(format!("{run_id}.json"));
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
    worker.call("open", json!({"cwd":base,"team":team,"resume":opened["session_id"]})).unwrap();
    assert!(wait_for(|| api.calls() >= 3, 5000));
    assert!(api.body(2)["messages"].as_array().unwrap().iter()
        .any(|m| m["role"] == "tool" && m["tool_call_id"] == "publish-0"), "recovery lost the original tool result");
    let entries = worker.entries();
    eprintln!("entries after kill/restart: {entries}");
    assert_eq!(entries.as_array().unwrap().len(), 1, "a committed model action was replayed with a new tool_call_id");
    }
}

#[test]
fn review_close_must_stop_running_shell_before_unlocking() {
    let _env = env_guard("review-shell-close");
    let cwd = isolated_project("shell-close");
    let api = FakeOpenAi::start(|_, _| (200, tool_call_response("shell-close", "shell", json!({
        "command":"touch started; sleep 2; printf late > after-close.txt", "timeout":10
    }))));
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
    let cwd = isolated_project("shell-timeout");
    let control = teamagents_engine::tools::shell_run("printf works", &cwd, 5, false, None).unwrap();
    assert_eq!(control, "works", "real sandbox must be working for the probe");
    let api = FakeOpenAi::start(|_, index| {
        if index == 0 { (200, tool_call_response("shell-1", "shell", json!({
            "command":"touch started; sleep 2; printf late > after-timeout.txt", "timeout":10
        }))) } else { (200, text_response("done")) }
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
fn review_unknown_external_effect_is_not_replayed_after_crash() {
    let _env = env_guard("review-unknown-external");
    let base = isolated_project("unknown-external");
    let api = FakeOpenAi::start(|_, _| (200, tool_call_response("external-1", "shell", json!({
        "command":"printf x >> count.txt; sleep 30; touch late.txt", "timeout":40
    }))));
    let team = worker_files(&base, Some(&api));
    let mut spec: Json = serde_json::from_slice(&std::fs::read(&team).unwrap()).unwrap();
    spec["agents"][0]["tool_bindings"] = json!(["shell"]);
    std::fs::write(&team, spec.to_string()).unwrap();
    let mut worker = ProbeWorker::spawn(&base);
    let opened = worker.call("open", json!({"cwd":base,"team":team})).unwrap();
    worker.call("user_message", json!({"text":"run once"})).unwrap();
    assert!(wait_for(|| base.join("count.txt").exists(), 5000), "real sandbox never executed");
    worker.kill();
    let mut worker = ProbeWorker::spawn(&base);
    worker.call("open", json!({"cwd":base,"team":team,"resume":opened["session_id"]})).unwrap();
    let state = worker.call("call", json!({"method":"state", "params":{}})).unwrap();
    assert_eq!(state["runs"][0]["status"], "OUTCOME_UNKNOWN", "{state}");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(api.calls(), 1, "uncertain external effect must not trigger a new model/tool call");
    assert_eq!(std::fs::read_to_string(base.join("count.txt")).unwrap(), "x");
    assert!(!base.join("late.txt").exists());
}

#[test]
fn review_executor_refreshes_workspace_and_revoked_bindings() {
    let _env = env_guard("review-executor-revision");
    let cwd = isolated_project("executor-revision");
    let api = FakeOpenAi::start(|_, index| {
        (200, match index {
            0 | 2 => tool_call_response(&format!("write-{index}"), "write_file", json!({"path":"result.txt", "content":index.to_string()})),
            4 => tool_call_response("revoked", "write_file", json!({"path":"revoked.txt", "content":"bad"})),
            _ => text_response("done"),
        })
    });
    let opened = open_chat_session(&cwd, &api, &["files"], UserConfig::default());
    opened.runtime.start();
    opened.runtime.user_message("first", false).unwrap();
    assert!(opened.runtime.settle(5));
    let patch = |id: &str, revision: i64, changes: Json| {
        let receipt = submit(&opened.core, id, "leader", "apply_topology_patch", json!({
            "base_revision":revision,"operations":[{"op":"update_agent","agent_id":"leader","changes":changes}]
        }));
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
    assert!(api.body(5)["messages"].as_array().unwrap().iter().any(|m|
        m["role"] == "tool" && m["tool_call_id"] == "revoked" && m["content"].as_str().unwrap_or("").contains("not bound")
    ));
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
    })).unwrap();
    let view = json!({"inbox_delta":[], "delivery_ids":[]});
    let gateway = ToolGateway::new(core.clone(), "leader", "finished",
        ApprovalGate::new(core.clone(), PermissionPolicy::default()), None);
    let first = chat_runner(&core, &agent, profile(&api.base_url(), "openai", json!({}), 0), "/tmp");
    assert_eq!(first.start_or_resume(&run, &view, &gateway, &Json::Null).status, TurnStatus::Completed);
    first.close();
    let restored = chat_runner(&core, &agent, profile(&api.base_url(), "openai", json!({}), 0), "/tmp");
    let restored_gateway = ToolGateway::new(core.clone(), "leader", "finished",
        ApprovalGate::new(core, PermissionPolicy::default()), None);
    assert_eq!(restored.reconcile(&run), Some(TurnStatus::Queued));
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
    })).unwrap();
    let view = json!({"inbox_delta":[], "delivery_ids":[]});
    let build = || chat_runner(&core, &agent, profile(&api.base_url(), "openai", json!({}), 0), "/tmp");
    let gateway = || ToolGateway::new(core.clone(), "leader", "journal",
        ApprovalGate::new(core.clone(), PermissionPolicy::default()), None);
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
        std::fs::write(&cp_path, checkpoint.to_string()).unwrap();
        std::fs::write(&tree_path, if renamed { tree.to_string() }
            else { json!({"ctx:leader:1":{"nodes":[], "leaf":null, "rewind_epoch":0}}).to_string() }).unwrap();
        let restored = build();
        let outcome = restored.start_or_resume(&run, &view, &gateway(), &Json::Null);
        assert_eq!(outcome.status, TurnStatus::Completed, "renamed={renamed}: {outcome:?}");
        assert_eq!(outcome.reply_text.as_deref(), Some("durable reply"));
        let actual: Json = serde_json::from_slice(&std::fs::read(&tree_path).unwrap()).unwrap();
        assert_eq!(actual, tree, "journal replay must not duplicate nodes");
        assert_eq!(api.calls(), 1, "saved final response must not be requested again");
        restored.close();
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
    std::fs::write(dir.join("chat_history.json"), json!({"ctx:leader:1":[
        {"role":"user","content":"legacy input"}, {"role":"assistant","content":"legacy reply"}
    ]}).to_string()).unwrap();
    opened.runtime.start();
    for text in ["first", "second"] {
        opened.runtime.user_message(text, false).unwrap();
        assert!(opened.runtime.settle(5));
    }
    opened.close();
    let tree: Json = serde_json::from_slice(&std::fs::read(dir.join("chat_tree.json")).unwrap()).unwrap();
    let replies: Vec<_> = tree["ctx:leader:1"]["nodes"].as_array().unwrap().iter()
        .filter(|n| n["message"]["role"] == "assistant").map(|n| n["message"]["content"].as_str().unwrap()).collect();
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
    let cwd = isolated_project("model-cancel");
    let api = FakeOpenAi::start(|_, index| (200, if index == 0 {
        tool_call_response("shell-1", "shell", json!({"command":"touch started; sleep 2; echo BAD > late", "timeout":10}))
    } else { text_response("done") }));
    let replacement = FakeOpenAi::start(|_, _| (200, json!({"content":[{"type":"text","text":"replacement done"}]})));
    let mut catalog = UserConfig::default();
    catalog.models.insert("replacement".into(), ModelProfile {
        provider: "anthropic".into(), model: "replacement-model".into(), context_window: Some(200000),
        ..profile(&replacement.base_url(), "anthropic", json!({"max_tokens":4321}), 0)
    });
    let opened = open_chat_session(&cwd, &api, &["shell"], catalog);
    opened.runtime.start();
    opened.runtime.user_message("run shell", false).unwrap();
    assert!(wait_for(|| cwd.join("started").exists(), 5000), "real sandbox did not start");
    let run = runs(&opened.core).into_iter().find(|r| r.status == TurnStatus::Running).unwrap();
    opened.set_model_selection("leader", Some("replacement".into()), None, Some("HIGH".into())).unwrap();
    let receipt = submit(&opened.core, "cancel", "user", "cancel_run", json!({"run_id":run.run_id}));
    assert!(receipt.ok);
    assert!(wait_for(|| runs(&opened.core).iter().any(|r| r.run_id == run.run_id && r.status == TurnStatus::Cancelled), 5000));
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
        cwd: Some(cwd), session_id: Some("late-compact".into()), catalog: Some(catalog),
        initial_spec: Some(json!({"leader_id":"leader", "agents":[agent_json("leader", "leader", &[])]})),
        ..Default::default()
    }).unwrap();
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
        },
        // after compaction the model asks for the covered tool output back
        2 | 4 => {
            // Discover the pointer from the request, as a stateless model must.
            let content = body["messages"].as_array().unwrap().iter()
                .filter_map(|m| m["content"].as_str()).find(|s| s.contains("Tool output index")).unwrap();
            let id = content.lines().find_map(|l| l.strip_prefix("- ").and_then(|l| l.split_once(": shell").map(|(id, _)| id))).unwrap();
            (200, tool_call_with_usage(&format!("read-{index}"), "read_history", json!({"tool_call_id": id}), if index == 2 { 10_000 } else { 500 }))
        },
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
                rows.iter().any(|r| r.status == TurnStatus::Completed)
                    && !rows.iter().any(|r| r.status.is_active())
            },
            15_000
        ),
        "the turn completes through compaction and readback"
    );
    assert_eq!(server.calls(), 6, "tool call -> summary -> read_history -> second summary -> read_history -> final");

    // call 1 is the summary request: the compactor saw the real tool output
    let summary_request = server.body(1)["messages"][0]["content"].as_str().unwrap_or("").to_string();
    assert!(summary_request.contains("compacting an agent conversation"), "summary prompt, got: {}", &summary_request[..summary_request.len().min(120)]);
    assert!(summary_request.contains("executed"), "compactor input includes the tool output");

    // call 2 runs on the compacted history: summary present, output gone
    let compacted = server.body(2).to_string();
    assert!(compacted.contains("SUMMARY: ran shell"), "summary carried into the wire history");
    assert!(!compacted.contains("\"executed\""), "original tool output compacted away: {}", &compacted[..compacted.len().min(400)]);

    // Even after a second compaction, the original output remains discoverable.
    assert!(server.body(4).to_string().contains("- call-1: shell"));
    let recovered = server.body(5).to_string();
    assert!(recovered.contains("executed"), "read_history returned the original output");

    // shell actually ran once; read_history never reaches the executor
    let called = tool_calls.lock().unwrap().clone();
    assert_eq!(called.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(), vec!["shell"]);
}
