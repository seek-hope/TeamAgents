//! R2-P1 对照证据:同一罐装 SSE 响应同时驱动旧 ChatRunner 与新 kernel 参考循环,
//! 比较请求构造与工具回执行为。两侧差异必须是已记录的设计差:旧路径携带团队
//! 工具与 <teamagents_worker> 系统信封;新参考循环携带 finish/read_history
//! 内建工具与参考指令。工具结果进入下一轮请求的行为必须一致。

mod support;

use serde_json::{json, Value as Json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::{core_with_spec, isolated_state_home as env_guard};
use teamagents_core::kernel::KernelProfile;
use teamagents_core::models::{ModelProfile, TurnRun, TurnStatus, UserConfig};
use teamagents_engine::bound::BoundTools;
use teamagents_engine::chat::ChatRunner;
use teamagents_engine::gateway::{ApprovalGate, PermissionPolicy, ToolGateway};
use teamagents_engine::providers::chat_completions::ChatCompletions;
use teamagents_engine::reference::{basic_tool_schemas, run_reference, ReferenceConfig, ReferenceEnd};
use teamagents_engine::runtime::{AgentRunner, Notify};

struct FakeServer {
    port: u16,
    bodies: Arc<Mutex<Vec<Json>>>,
}

impl FakeServer {
    fn start(responses: Vec<Json>) -> FakeServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let bodies: Arc<Mutex<Vec<Json>>> = Arc::new(Mutex::new(vec![]));
        let recorded = bodies.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let raw = read_request(&mut stream);
                let request: Json = serde_json::from_str(&raw).unwrap_or(Json::Null);
                recorded.lock().unwrap().push(request.clone());
                let index = calls.fetch_add(1, Ordering::SeqCst);
                let Some(payload) = responses.get(index) else { return };
                let payload = payload.to_string();
                let head = format!(
                    "HTTP/1.1 200 X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    payload.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(payload.as_bytes());
                let _ = stream.flush();
            }
        });
        FakeServer { port, bodies }
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn body(&self, index: usize) -> Json {
        self.bodies.lock().unwrap()[index].clone()
    }
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
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let length: usize = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
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

fn script() -> Vec<Json> {
    vec![
        json!({"id":"x","choices":[{"message":{"role":"assistant","content":null,"tool_calls":[
            {"id":"call1","type":"function","function":{"name":"shell","arguments":"{\"command\":\"echo COMPARE-MARKER\"}"}}]}}],
            "usage":{"prompt_tokens":11,"completion_tokens":5,"total_tokens":16}}),
        json!({"id":"x","choices":[{"message":{"role":"assistant","content":"final answer"}}],
            "usage":{"prompt_tokens":21,"completion_tokens":7,"total_tokens":28}}),
    ]
}

fn tool_names(body: &Json) -> Vec<String> {
    body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap_or(t["name"].as_str().unwrap_or("")).to_string())
        .collect()
}

fn tool_result_contents(body: &Json) -> Vec<String> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "tool")
        .filter_map(|m| m["content"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn old_and_new_paths_construct_comparable_requests() {
    let _env = env_guard("rebuild-p1-compare");
    // ---- old path: ChatRunner through the team gateway ----
    let old_server = FakeServer::start(script());
    let session = "rebuild-p1-compare";
    let agent = json!({"id":"worker","name":"worker","role":"worker","runtime_kind":"deepagents",
        "model_profile":"m","tool_bindings":["files","shell"]});
    let core = core_with_spec(
        session,
        json!({"leader_id":"leader","agents":[
            {"id":"leader","name":"leader","role":"leader","runtime_kind":"deepagents","model_profile":"m","tool_bindings":[]},
            agent.clone()]}),
    );
    let profile = ModelProfile {
        provider: "openai".into(),
        protocol: "openai".into(),
        model: "test".into(),
        base_url: Some(old_server.base()),
        api_key_env: None,
        timeout: 30,
        max_retries: 0,
        generation_options: Default::default(),
        context_window: None,
        codex_profile: None,
    };
    let bound = BoundTools::load(&UserConfig::default(), &["files".to_string(), "shell".to_string()]).unwrap();
    let runner =
        ChatRunner::new(&agent, profile, Some("/tmp".into()), Notify::new(core.clone()), bound, vec![], (false, false));
    let run: TurnRun = serde_json::from_value(json!({
        "run_id":"compare-run","session_id":session,"agent_id":"worker","task_id":"task-1","goal_id":null,
        "status":"QUEUED","config_revision":1,"topology_revision":1,"input_delivery_ids":[],
        "context_ref":"ctx:worker","external_turn_id":null,"cancel_requested":false,
        "waiting_on":[],"created_at":0,"updated_at":0,
    }))
    .unwrap();
    type Exec = Arc<dyn Fn(&str, &Json) -> Result<Json, String> + Send + Sync>;
    let executor: Exec = Arc::new(teamagents_engine::tools::member_executor(
        std::path::PathBuf::from("/tmp"),
        UserConfig::default(),
        vec!["files".to_string(), "shell".to_string()],
        None,
    ));
    let gateway = ToolGateway::new(
        core.clone(),
        "worker",
        "compare-run",
        ApprovalGate::new(core.clone(), PermissionPolicy::default()),
        Some(executor),
    );
    let view = json!({"agent_id":"worker","assignment":[{
        "task_id":"task-1","description":"echo the marker","acceptance":"marker printed","requester":"leader"}],
        "inbox_delta":[],"permitted_shared_delta":[],"relevant_topology":{"revision":1}});
    let outcome = runner.start_or_resume(&run, &view, &gateway, &json!({"reason":"new_input"}));
    assert_eq!(outcome.status, TurnStatus::Completed, "{outcome:?}");
    runner.close();

    // ---- new path: kernel reference loop ----
    let new_server = FakeServer::start(script());
    let workspace = std::env::temp_dir().join(format!("teamagents-rebuild-p1-compare-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&workspace).unwrap();
    let trace_dir = workspace.join("trace");
    let provider = ChatCompletions::new(new_server.base(), "", Duration::from_secs(30)).unwrap();
    let config = ReferenceConfig {
        workspace: workspace.clone(),
        artifacts: None,
        shell_state: None,
        permissions: "approved_scope".into(),
        profile: KernelProfile {
            model: "test".into(),
            instructions: "reference agent".into(),
            tools: basic_tool_schemas(false),
            options: json!({}),
            context_window: Some(1_000_000),
        },
        catalog: UserConfig::default(),
        bindings: vec![],
        max_steps: 6,
        max_retries: 0,
        deadline: Some(Duration::from_secs(60)),
        trace_dir,
        run_id: "compare".into(),
    };
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let reference = rt.block_on(run_reference(&provider, config, "echo the marker", |_| {})).unwrap();
    assert!(matches!(reference.end, ReferenceEnd::Reply(_)));

    // ---- 对照:两轮请求都是 chat-completions 线形;工具结果都进入第二轮 ----
    let old_first = old_server.body(0);
    let old_second = old_server.body(1);
    let new_first = new_server.body(0);
    let new_second = new_server.body(1);
    for body in [&old_first, &new_first] {
        assert_eq!(body["model"], "test");
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["role"], "system");
        assert!(body["messages"].as_array().unwrap().iter().any(|m| m["role"] == "user"));
    }
    // 设计差:旧路径有团队工具,新参考有 finish/read_history;基础工具两侧都在
    let old_tools = tool_names(&old_first);
    let new_tools = tool_names(&new_first);
    for basic in ["shell", "read_file", "write_file", "glob", "grep", "ls"] {
        assert!(old_tools.contains(&basic.to_string()), "old missing {basic}");
        assert!(new_tools.contains(&basic.to_string()), "new missing {basic}");
    }
    assert!(old_tools.iter().any(|t| t == "complete_task" || t == "send_message"));
    assert!(new_tools.contains(&"finish".to_string()));
    assert!(new_tools.contains(&"read_history".to_string()));
    assert!(!old_tools.contains(&"finish".to_string()));
    // 工具结果:同一命令输出以相同 tool 消息形状进入第二轮请求
    let old_results = tool_result_contents(&old_second);
    let new_results = tool_result_contents(&new_second);
    assert_eq!(old_results.len(), 1, "old second body: {old_second}");
    assert_eq!(new_results.len(), 1, "new second body: {new_second}");
    assert!(old_results[0].contains("COMPARE-MARKER"), "old: {}", old_results[0]);
    assert_eq!(old_results[0], new_results[0], "tool result content must match across paths");
    // 两侧 assistant 轮都带 tool_calls 引用同一 call_id
    let old_assistant = old_second["messages"].as_array().unwrap().iter().find(|m| m["role"] == "assistant").unwrap();
    let new_assistant = new_second["messages"].as_array().unwrap().iter().find(|m| m["role"] == "assistant").unwrap();
    assert_eq!(old_assistant["tool_calls"][0]["id"], "call1");
    assert_eq!(new_assistant["tool_calls"][0]["id"], "call1");
}
