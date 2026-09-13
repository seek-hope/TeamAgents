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
    }
}

fn chat_runner(core: &Arc<CoreClient>, agent: &Json, profile: ModelProfile, workdir: &str) -> Arc<ChatRunner> {
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
        Notify::new(core.clone()),
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
    let notify = Notify::new(core.clone());
    let approvals = ApprovalGate::new(core.clone(), policy);
    let runtime = Runtime::new(core.clone(), notify, approvals, executor, None, limits);
    runtime.add_runner(agent_id, runner);
    runtime.start();
    runtime
}

fn recording_executor() -> (ToolExecutor, Arc<Mutex<Vec<(String, Json)>>>) {
    let calls: Arc<Mutex<Vec<(String, Json)>>> = Arc::new(Mutex::new(vec![]));
    let sink = calls.clone();
    let executor: ToolExecutor = Arc::new(move |_agent: &str, tool: &str, args: &Json| {
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

// ---- tests -----------------------------------------------------------------

/// F-3: a once approval must be found again when the model re-sends the call
/// with a fresh tool_call_id, the operation must run, and the row must be
/// consumed (EXPIRED) — then the turn finishes.
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
    let executor: ToolExecutor = Arc::new(|_agent: &str, tool: &str, _args: &Json| Err(format!("no executor for {tool}")));
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
