//! 退役（R29，2026-09-24）：本文件覆盖 v1 后端（chat/codex/session/worker/审查树）。
//! v1 入口已不可达；等价覆盖在 v2：`v2_driver`/`v2_supervisor`/`v2_daemon`/`v2_mcp`/`v2_spawn_failure`、
//! `review/eval/r2-p6`（性能）与 `review/eval/r2-p5` 的真实供应商验收。文件待随模块删除。
#![cfg(any())]
//! T8/T21: crash windows — a killed session is reconciled on restart and
//! replayed actions stay exactly-once.

use serde_json::{json, Value as Json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod support;

struct CloseRuntime(Arc<teamagents_engine::runtime::Runtime>);

impl Drop for CloseRuntime {
    fn drop(&mut self) {
        self.0.close();
    }
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    responses: Receiver<Json>,
    next_id: u64,
    stderr: PathBuf,
}

impl Worker {
    fn spawn(state_home: &std::path::Path, config_home: &std::path::Path) -> Worker {
        let stderr = state_home.with_extension("worker-stderr.log");
        let log = std::fs::OpenOptions::new().create(true).append(true).open(&stderr).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .arg("serve")
            .env("XDG_STATE_HOME", state_home)
            .env("XDG_CONFIG_HOME", config_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log))
            .spawn()
            .expect("spawn engine");
        let stdout = child.stdout.take().unwrap();
        let stdin = child.stdin.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(message) = serde_json::from_str::<Json>(&line) {
                    if message.get("push").is_none() {
                        let _ = tx.send(message);
                    }
                }
            }
        });
        Worker { child, stdin, responses: rx, next_id: 1, stderr }
    }

    fn call(&mut self, method: &str, params: Json) -> Result<Json, String> {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(self.stdin, "{}", json!({"id": id, "method": method, "params": params})).unwrap();
        self.stdin.flush().unwrap();
        loop {
            let message = self.responses.recv_timeout(std::time::Duration::from_secs(30)).expect("worker reply");
            if message.get("id").and_then(|v| v.as_u64()) != Some(id) {
                continue;
            }
            return match message.get("error").and_then(|v| v.as_str()) {
                Some(error) => Err(error.to_string()),
                None => Ok(message.get("result").cloned().unwrap_or(Json::Null)),
            };
        }
    }

    fn state(&mut self) -> Json {
        self.call("call", json!({"method": "state", "params": {}})).expect("state")
    }

    fn shared_entries(&mut self) -> Vec<String> {
        let reply = self
            .call("call", json!({"method": "shared_entries", "params": {"space_ids": ["main"]}}))
            .expect("shared_entries");
        reply["entries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|e| e.get("content").and_then(|v| v.as_str()).map(str::to_string))
            .collect()
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn close(mut self) {
        let _ = self.call("close", json!({}));
        let _ = self.child.wait();
    }

    fn crash(&mut self) {
        self.child.kill().expect("SIGKILL worker");
        assert_eq!(self.child.wait().unwrap().signal(), Some(9));
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // A failed assertion must not leave an engine holding the session lock.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Plain {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    text: String,
}

impl Plain {
    fn spawn(root: &Path, session: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .args(["--plain", "--resume", session, "--cwd"])
            .arg(root.join("project"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(std::fs::File::create(root.join("plain-stderr.log")).unwrap()))
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut plain = Self { child, stdin, lines, text: String::new() };
        plain.wait_line(|line| line.starts_with("session:"));
        plain
    }

    fn send(&mut self, text: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{text}").unwrap();
        stdin.flush().unwrap();
    }

    fn wait_line(&mut self, ready: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let line = self
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| panic!("plain output stalled: {error}; output:\n{}", self.text));
            self.text.push_str(&line);
            self.text.push('\n');
            if ready(&line) {
                return line;
            }
        }
    }

    fn close(&mut self) {
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "{status}");
                return;
            }
            assert!(Instant::now() < deadline, "plain did not exit after EOF");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Plain {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn run_statuses(state: &Json) -> Vec<String> {
    state["runs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|r| r.get("status").and_then(|v| v.as_str()).map(str::to_string))
        .collect()
}

#[test]
fn t8_killed_turn_is_reconciled_and_stays_exactly_once() {
    let root = std::env::temp_dir().join(format!("ta-recovery-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (state_home, config_home) = (root.join("state"), root.join("config"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.m]\nprovider = \"openai\"\nprotocol = \"openai\"\nmodel = \"test\"\n",
    )
    .unwrap();
    let scripts = json!({"leader": [
        ["call", "publish_shared", {"space_id": "main", "kind": "finding", "content": "once-only"}],
        ["sleep", 30],
        ["end"],
    ]});

    let mut worker = Worker::spawn(&state_home, &config_home);
    let opened = worker.call("open", json!({"cwd": "/tmp", "scripts": scripts})).expect("open");
    let session_id = opened["session_id"].as_str().unwrap().to_string();
    worker.call("user_message", json!({"text": "publish then hang"})).expect("user_message");

    // wait until the member actually published and is mid-turn, then kill it
    let published = {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let state = worker.state();
            if !worker.shared_entries().is_empty() && run_statuses(&state).contains(&"RUNNING".to_string()) {
                break state;
            }
            assert!(std::time::Instant::now() < deadline, "member never started: {state}");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    };
    assert_eq!(worker.shared_entries(), vec!["once-only".to_string()]);
    let _ = published;
    worker.kill();
    eprintln!("killed mid-turn (run left RUNNING in {session_id})");

    // restart: the parked turn must be reconciled (requeued and re-run), never
    // left RUNNING forever, and its already-applied action must not repeat
    let mut worker = Worker::spawn(&state_home, &config_home);
    worker.call("open", json!({"cwd": "/tmp", "resume": session_id, "scripts": scripts})).expect("resume");
    let reconciled = {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let state = worker.state();
            let statuses = run_statuses(&state);
            if statuses.iter().any(|s| s == "RUNNING") {
                break state;
            }
            assert!(std::time::Instant::now() < deadline, "turn was not reconciled: {statuses:?}");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    };
    let _ = reconciled;
    assert_eq!(
        worker.shared_entries(),
        vec!["once-only".to_string()],
        "the replayed step must not publish twice (action receipt dedup, T21)"
    );

    // stop the resumed turn and settle
    let run_id = reconciled["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r.get("status").and_then(|v| v.as_str()) == Some("RUNNING"))
        .and_then(|r| r.get("run_id").and_then(|v| v.as_str()))
        .unwrap()
        .to_string();
    worker
        .call(
            "submit",
            json!({"action": {"action_id": "cancel-1", "kind": "cancel_run",
                                          "payload": {"run_id": run_id}}}),
        )
        .expect("cancel");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let statuses = run_statuses(&worker.state());
        if !statuses.iter().any(|s| s == "RUNNING") {
            assert!(statuses.contains(&"CANCELLED".to_string()), "{statuses:?}");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "cancel did not settle: {statuses:?}");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    worker.close();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn t22_goal_turn_budget_is_enforced() {
    // A goal may not spend more than limits.max_turns_per_goal model turns;
    // exceeding it is an explicit LIMIT_REACHED event, not a silent stall.
    let root = std::env::temp_dir().join(format!("ta-budget-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (state_home, config_home) = (root.join("state"), root.join("config"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nprotocol = \"openai\"\nmodel = \"test\"\n",
    )
    .unwrap();
    let spec_path = root.join("team.json");
    std::fs::write(
        &spec_path,
        serde_json::to_string(&json!({
            "leader_id": "leader",
            "agents": [{"id": "leader", "name": "Leader", "role": "leader",
                        "runtime_kind": "deepagents", "model_profile": "leader_main",
                        "tool_bindings": ["files"]}],
            "limits": {"max_turns_per_goal": 1},
        }))
        .unwrap(),
    )
    .unwrap();
    let scripts = json!({"leader": [["end"]]});
    let mut worker = Worker::spawn(&state_home, &config_home);
    worker.call("open", json!({"cwd": "/tmp", "team": spec_path.to_string_lossy(), "scripts": scripts})).expect("open");

    for turn in 0..3 {
        let _ = worker.call("user_message", json!({"text": format!("turn {turn}")}));
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let state = worker.state();
    let kinds: Vec<String> = state["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|e| e.get("kind").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    assert!(kinds.contains(&"limit_reached".to_string()), "budget must report LIMIT_REACHED: {kinds:?}");
    let runs = state["runs"].as_array().cloned().unwrap_or_default().len();
    assert!(runs <= 2, "the budget stops extra turns (runs={runs})");
    worker.close();
    let _ = std::fs::remove_dir_all(&root);
}

/// Hold individual HTTP responses at a real process boundary; the test decides
/// when a model response is delivered and when the worker is killed.
struct ModelRequest {
    body: Json,
    stream: TcpStream,
}

impl ModelRequest {
    fn reply(self, message: Json) {
        self.respond(
            200,
            json!({"choices":[{"message":message}],
            "usage":{"prompt_tokens":100,"completion_tokens":10,"total_tokens":110}}),
        );
    }

    fn respond(mut self, status: u16, body: Json) {
        let body = body.to_string();
        write!(self.stream,
            "HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()).unwrap();
    }

    fn tools(&self, id: &str) -> Vec<&Json> {
        self.body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "tool" && message["tool_call_id"] == id)
            .collect()
    }

    fn user_messages_containing(&self, text: &str) -> usize {
        self.body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| {
                message["role"] == "user" && message["content"].as_str().is_some_and(|content| content.contains(text))
            })
            .count()
    }
}

struct Model {
    url: String,
    requests: Receiver<ModelRequest>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Model {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let (tx, requests) = channel();
        let handle = std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                let (stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("model accept: {error}"),
                };
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                stream.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0;
                let mut line = String::new();
                loop {
                    line.clear();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':') {
                        if key.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse::<usize>().unwrap();
                        }
                    }
                }
                assert!(length > 0 && length < 2_000_000);
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                if tx.send(ModelRequest { body: serde_json::from_slice(&body).unwrap(), stream }).is_err() {
                    break;
                }
            }
        });
        Self { url, requests, stop, handle: Some(handle) }
    }

    fn next(&self) -> ModelRequest {
        self.requests.recv_timeout(Duration::from_secs(10)).expect("next model request")
    }

    fn assert_quiet(&self) {
        assert!(
            matches!(
                self.requests.recv_timeout(Duration::from_millis(300)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ),
            "no additional model request is allowed at this boundary"
        );
    }
}

impl Drop for Model {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let result = handle.join();
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
    }
}

fn chat_fixture(model: &Model, tag: &str) -> support::TestEnv {
    let mut env = support::isolated_state_home(tag);
    env.set("XDG_STATE_HOME", env.join("state"));
    std::fs::create_dir_all(env.join("project")).unwrap();
    std::fs::create_dir_all(env.join("config/teamagents")).unwrap();
    std::fs::write(
        env.join("config/teamagents/config.toml"),
        format!(
            "[models.m]\nprovider='openai'\nprotocol='openai'\nmodel='local-fixture'\nbase_url='{}'\n\
             api_key_env='TA_CHAT_RECOVERY_KEY'\nmax_retries=0\ntimeout=20\n",
            model.url
        ),
    )
    .unwrap();
    env.set("TA_CHAT_RECOVERY_KEY", "test-only");
    env
}

fn open_chat(root: &Path, id: &str) -> Worker {
    open_chat_with_mode(root, id, false)
}

fn open_chat_with_mode(root: &Path, id: &str, full_auto: bool) -> Worker {
    let mut worker = Worker::spawn(&root.join("state"), &root.join("config"));
    worker
        .call(
            "open",
            json!({
                "cwd": root.join("project"), "resume": id, "fullAuto": full_auto,
                "initial_spec": {
                    "leader_id":"leader", "agents":[{
                        "id":"leader", "name":"Leader", "role":"leader", "runtime_kind":"deepagents",
                        "model_profile":"m", "tool_bindings":["files","shell"]
                    }]
                }
            }),
        )
        .unwrap();
    worker
}

fn wait_state(worker: &mut Worker, ready: impl Fn(&Json) -> bool) -> Json {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let state = worker.state();
        if ready(&state) {
            return state;
        }
        assert!(Instant::now() < deadline, "state did not converge: {state}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn checkpoint(root: &Path, id: &str, run_id: &str) -> PathBuf {
    root.join("state/teamagents/sessions").join(id).join("members/leader/turns").join(format!("{run_id}.json"))
}

fn shell_message(id: &str, command: &str, network: bool) -> Json {
    json!({"role":"assistant","content":null,"tool_calls":[{
        "id":id,"type":"function","function":{"name":"shell",
            "arguments":json!({"command":command,"network":network,"timeout":15}).to_string()}
    }]})
}

#[test]
fn chat_cold_resume_keeps_tool_results_and_unconsumed_supplements() {
    if !teamagents_engine::tools::bwrap_available() {
        eprintln!("skipped: bwrap is unavailable");
        return;
    }
    let model = Model::start();
    let env = chat_fixture(&model, "chat-cold-result");
    let mut worker = open_chat(&env, "cold-result");
    worker.call("user_message", json!({"text":"ORIGINAL_REQUEST_MARKER"})).unwrap();
    let first = model.next();
    assert_eq!(first.user_messages_containing("ORIGINAL_REQUEST_MARKER"), 1);
    first.reply(shell_message("write-once", "printf 'once\\n' >> effect.txt", false));
    let interrupted_request = model.next();
    let recorded = interrupted_request.tools("write-once");
    assert_eq!(recorded.len(), 1);
    let recorded = recorded[0].clone();
    let result: Json = serde_json::from_str(recorded["content"].as_str().unwrap()).unwrap();
    assert!(result.get("error").is_none() && result.get("output").is_some(), "{result}");
    assert_eq!(std::fs::read_to_string(env.join("project/effect.txt")).unwrap(), "once\n");
    worker.call("user_message", json!({"text":"LATE_SUPPLEMENT_MARKER","supplement":true})).unwrap();
    let before = worker.state();
    let run_id = before["runs"][0]["run_id"].as_str().unwrap().to_string();
    let saved: Json =
        serde_json::from_slice(&std::fs::read(checkpoint(&env, "cold-result", &run_id)).unwrap()).unwrap();
    assert!(saved["pending_external"].is_null());
    assert!(!saved["history"].to_string().contains("LATE_SUPPLEMENT_MARKER"));
    worker.crash();
    drop(interrupted_request);
    drop(worker);

    let mut resumed = open_chat(&env, "cold-result");
    let request = model.next();
    assert_eq!(request.tools("write-once"), vec![&recorded]);
    assert_eq!(request.user_messages_containing("ORIGINAL_REQUEST_MARKER"), 1);
    assert_eq!(request.user_messages_containing("LATE_SUPPLEMENT_MARKER"), 1);
    request.reply(json!({"role":"assistant","content":"RECOVERED_COMPLETION"}));
    let state = wait_state(&mut resumed, |state| state["runs"][0]["status"] == "COMPLETED");
    assert_eq!(state["runs"].as_array().unwrap().len(), 1, "supplement belongs to the same run");
    assert_eq!(state["runs"][0]["run_id"], run_id);
    assert!(state["events"].to_string().contains("RECOVERED_COMPLETION"));
    assert_eq!(std::fs::read_to_string(env.join("project/effect.txt")).unwrap(), "once\n");
    model.assert_quiet();
    resumed.close();
}

#[test]
fn chat_cold_resume_preserves_pending_approval_and_consumes_it_once() {
    if !teamagents_engine::tools::bwrap_available() {
        eprintln!("skipped: bwrap is unavailable");
        return;
    }
    let model = Model::start();
    let env = chat_fixture(&model, "chat-cold-approval");
    let mut worker = open_chat(&env, "cold-approval");
    let command = "printf 'approved once\\n' >> effect.txt";
    worker.call("user_message", json!({"text":"request a network-capable local command"})).unwrap();
    model.next().reply(shell_message("needs-approval", command, true));
    let parked = wait_state(&mut worker, |state| state["runs"][0]["status"] == "WAITING_APPROVAL");
    let approval_id = parked["pending_approvals"][0]["approval_id"].as_str().unwrap().to_string();
    let run_id = parked["runs"][0]["run_id"].clone();
    assert!(!env.join("project/effect.txt").exists());
    worker.crash();
    drop(worker);

    let mut resumed = open_chat(&env, "cold-approval");
    assert_eq!(resumed.state()["pending_approvals"], parked["pending_approvals"]);
    model.assert_quiet();
    let receipt = resumed
        .call(
            "submit",
            json!({"action":{
                "action_id":"approve-once", "kind":"approval_decision",
                "payload":{"approval_id":approval_id,"decision":"once"}
            }}),
        )
        .unwrap();
    assert_eq!(receipt["ok"], true, "{receipt}");
    model.next().reply(shell_message("approved-retry", command, true));
    let request = model.next();
    let results = request.tools("approved-retry");
    assert_eq!(results.len(), 1);
    let result: Json = serde_json::from_str(results[0]["content"].as_str().unwrap()).unwrap();
    assert!(result.get("error").is_none() && result.get("output").is_some(), "{result}");
    request.reply(json!({"role":"assistant","content":"APPROVED_COMPLETION"}));
    let finished = wait_state(&mut resumed, |state| state["runs"][0]["status"] == "COMPLETED");
    assert_eq!(finished["runs"].as_array().unwrap().len(), 1);
    assert_eq!(finished["runs"][0]["run_id"], run_id);
    assert!(finished["pending_approvals"].as_array().unwrap().is_empty());
    assert_eq!(std::fs::read_to_string(env.join("project/effect.txt")).unwrap(), "approved once\n");
    let approval = resumed.call("call", json!({"method":"get_approval","params":{"approval_id":approval_id}})).unwrap();
    assert_eq!(approval["approval"]["status"], "EXPIRED");
    model.assert_quiet();
    resumed.close();
}

#[test]
fn chat_cold_resume_does_not_repeat_an_external_effect_without_a_receipt() {
    if !teamagents_engine::tools::bwrap_available() {
        eprintln!("skipped: bwrap is unavailable");
        return;
    }
    let model = Model::start();
    let env = chat_fixture(&model, "chat-cold-unknown");
    let mut worker = open_chat(&env, "cold-unknown");
    worker.call("user_message", json!({"text":"start an external operation"})).unwrap();
    model.next().reply(shell_message("uncertain-call", "printf 'started once\\n' >> effect.txt; sleep 12", false));
    assert!(support::wait_for(|| env.join("project/effect.txt").exists(), 5_000));
    let before = worker.state();
    let run_id = before["runs"][0]["run_id"].as_str().unwrap().to_string();
    let path = checkpoint(&env, "cold-unknown", &run_id);
    let original = std::fs::read(&path).unwrap();
    let saved: Json = serde_json::from_slice(&original).unwrap();
    assert_eq!(saved["pending_external"], "uncertain-call");
    worker.crash();
    drop(worker);

    let mut resumed = open_chat(&env, "cold-unknown");
    let state = wait_state(&mut resumed, |state| state["runs"][0]["status"] == "OUTCOME_UNKNOWN");
    assert_eq!(state["runs"].as_array().unwrap().len(), 1);
    assert_eq!(state["runs"][0]["run_id"], run_id);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::read_to_string(env.join("project/effect.txt")).unwrap(), "started once\n");
    model.assert_quiet();
    resumed.close();
}

#[test]
fn exec_reports_rejected_input_instead_of_reusing_the_previous_completed_goal() {
    let model = Model::start();
    let env = chat_fixture(&model, "exec-rejected-input");
    let mut worker = open_chat(&env, "exec-input");
    worker.call("user_message", json!({"text":"complete the previous goal"})).unwrap();
    model.next().reply(json!({"role":"assistant","content":null,"tool_calls":[{
        "id":"done","type":"function","function":{"name":"signal_done","arguments":"{\"summary\":\"old goal\"}"}
    }]}));
    model.next().reply(json!({"role":"assistant","content":"OLD_GOAL_DONE"}));
    wait_state(&mut worker, |state| state["session"]["goal_state"] == "done");
    worker.close();
    let db = rusqlite::Connection::open(env.join("state/teamagents/sessions/exec-input/team.db")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER refuse_input BEFORE INSERT ON events WHEN NEW.kind='user_message'
         BEGIN SELECT RAISE(ABORT, 'new input storage unavailable'); END;",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
        .args(["exec", "--json", "--resume", "exec-input", "--timeout", "3", "--check", "touch CHECK_SHOULD_NOT_RUN"])
        .arg("--cwd")
        .arg(env.join("project"))
        .arg("NEW_UNACCEPTED_GOAL")
        .env("XDG_STATE_HOME", env.join("state"))
        .env("XDG_CONFIG_HOME", env.join("config"))
        .output()
        .unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<Json> = text.lines().map(|line| serde_json::from_str(line).expect("JSONL output")).collect();
    assert_eq!(output.status.code(), Some(1), "{text}");
    let result = lines.last().expect("structured failure result");
    assert_eq!(result["type"], "result");
    assert_eq!(result["status"], "failed");
    assert_eq!(result["exit_code"], 1);
    assert!(result["runtime_errors"].to_string().contains("new input storage unavailable"), "{result}");
    assert_eq!(result["verification"], json!([]), "rejected input must not run delivery checks");
    assert!(!env.join("project/CHECK_SHOULD_NOT_RUN").exists());
    assert_eq!(db.query_row("SELECT goal_state FROM sessions", [], |row| row.get::<_, String>(0)).unwrap(), "done");
    model.assert_quiet();
}

#[test]
fn plain_rejects_uncommitted_input_and_accepts_a_new_request_after_repair() {
    let model = Model::start();
    let env = chat_fixture(&model, "plain-input-rejection");
    open_chat(&env, "plain-input").close();
    let db = rusqlite::Connection::open(env.join("state/teamagents/sessions/plain-input/team.db")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER refuse_plain_input BEFORE INSERT ON events WHEN NEW.kind='user_message'
         BEGIN SELECT RAISE(ABORT, 'plain input storage unavailable'); END;",
    )
    .unwrap();
    let mut plain = Plain::spawn(&env, "plain-input");
    plain.send("REJECTED_PLAIN_INPUT");
    let receipt = plain.wait_line(|line| {
        line.contains("input received") || line.contains("输入已接收") || line.contains("输入被拒绝")
    });
    assert!(receipt.contains("输入被拒绝") && receipt.contains("plain input storage unavailable"), "{receipt}");
    assert_eq!(db.query_row("SELECT COUNT(*) FROM turn_runs", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM events WHERE kind='user_message'", [], |row| row.get::<_, i64>(0)).unwrap(),
        0
    );
    model.assert_quiet();
    db.execute_batch("DROP TRIGGER refuse_plain_input").unwrap();
    plain.send("ACCEPTED_PLAIN_INPUT");
    let request = model.next();
    assert_eq!(request.user_messages_containing("REJECTED_PLAIN_INPUT"), 0);
    assert_eq!(request.user_messages_containing("ACCEPTED_PLAIN_INPUT"), 1);
    request.reply(json!({"role":"assistant","content":"PLAIN_REQUEST_COMPLETED"}));
    plain.wait_line(|line| line.contains("[Leader] PLAIN_REQUEST_COMPLETED"));
    model.assert_quiet();
    plain.close();
}

#[test]
fn plain_storage_failure_returns_control_and_recovers_without_resubmitting_input() {
    use teamagents_core::models::Task;
    use teamagents_core::storage::Store;

    for phase in ["prepare", "state"] {
        let model = Model::start();
        let env = chat_fixture(&model, "plain-storage-failure");
        let mut worker = open_chat(&env, "plain-storage");
        let mut spec = worker.state()["spec"].clone();
        spec["shared_spaces"] = json!([{"id":"main","readers":["leader"],"writers":["leader"]}]);
        worker.call("call", json!({"method":"save_spec","params":{"spec":spec}})).unwrap();
        worker.close();
        let mut plain = Plain::spawn(&env, "plain-storage");
        let store = Store::open(&env.join("state/teamagents/sessions/plain-storage/team.db")).unwrap();
        if phase == "prepare" {
            store
                .conn
                .execute("INSERT INTO shared_cursors VALUES('plain-storage','leader','main','broken')", [])
                .unwrap();
        } else {
            let task: Task = serde_json::from_value(json!({
                "task_id":"old-task","requester":"leader","assignee":"leader","description":"old work","status":"SUCCEEDED"
            })).unwrap();
            store.insert_task("plain-storage", &task).unwrap();
            store.conn.execute("UPDATE tasks SET result_refs='['", []).unwrap();
        }
        plain.send("ORIGINAL_PLAIN_INPUT");
        plain.wait_line(|line| line.contains("等待存储恢复") || line.contains("读取状态失败"));
        plain.send("status");
        plain.wait_line(|line| line.contains("成员 | 模型"));
        model.assert_quiet();
        if phase == "prepare" {
            store.conn.execute("UPDATE shared_cursors SET sequence=0", []).unwrap();
        } else {
            store.conn.execute("UPDATE tasks SET result_refs='[]'", []).unwrap();
        }
        let request = model.next();
        assert_eq!(request.user_messages_containing("ORIGINAL_PLAIN_INPUT"), 1);
        request.reply(json!({"role":"assistant","content":"RECOVERED_PLAIN_REPLY"}));
        assert!(support::wait_for(
            || {
                store.conn.query_row("SELECT status FROM turn_runs", [], |row| row.get::<_, String>(0)).unwrap()
                    == "COMPLETED"
            },
            5_000
        ));
        plain.send("status");
        plain.wait_line(|line| line.contains("[Leader] RECOVERED_PLAIN_REPLY"));
        assert_eq!(store.runs_for_session("plain-storage", &[]).unwrap().len(), 1);
        model.assert_quiet();
        plain.close();
    }
}

#[test]
fn returned_chat_outcome_survives_a_state_read_failure_without_another_model_call() {
    check_returned_outcome_after_read_failure("repair");
}

#[test]
fn returned_chat_outcome_survives_read_failure_across_close_and_sigkill() {
    for shutdown in ["close", "crash"] {
        check_returned_outcome_after_read_failure(shutdown);
    }
}

#[test]
fn returned_chat_outcome_survives_cancellation_during_read_failure_and_restart() {
    for shutdown in ["cancel", "cancel-crash"] {
        check_returned_outcome_after_read_failure(shutdown);
    }
}

#[test]
fn returned_chat_outcome_survives_cancellation_while_waiting_to_finalize() {
    for shutdown in ["cancel-finalize", "cancel-finalize-crash"] {
        check_returned_outcome_after_read_failure(shutdown);
    }
}

#[test]
fn returned_chat_outcome_survives_a_legacy_queued_cancellation() {
    for shutdown in ["cancel-legacy-crash", "cancel-legacy-auto-crash", "cancel-legacy-admission-crash"] {
        check_returned_outcome_after_read_failure(shutdown);
    }
}

fn check_returned_outcome_after_read_failure(shutdown: &str) {
    use teamagents_core::models::Task;
    use teamagents_core::storage::Store;

    for failed in [false, true] {
        let model = Model::start();
        let env = chat_fixture(&model, "returned-outcome-read-failure");
        let mut worker = open_chat(&env, "returned-outcome");
        worker.call("user_message", json!({"text":"PRESERVE_RETURNED_OUTCOME"})).unwrap();
        model.next().reply(json!({"role":"assistant","content":null,"tool_calls":[{
            "id":"write-once","type":"function","function":{"name":"write_file",
                "arguments":json!({"path":"effect.txt","content":"written once\n"}).to_string()}
        }]}));
        let request = model.next();
        assert_eq!(request.tools("write-once").len(), 1);
        assert_eq!(std::fs::read_to_string(env.join("project/effect.txt")).unwrap(), "written once\n");
        let run_id = worker.state()["runs"][0]["run_id"].as_str().unwrap().to_string();
        let store = Store::open(&env.join("state/teamagents/sessions/returned-outcome/team.db")).unwrap();
        let task: Task = serde_json::from_value(json!({
            "task_id":"old-task","requester":"leader","assignee":"leader","description":"prior work","status":"SUCCEEDED"
        })).unwrap();
        store.insert_task("returned-outcome", &task).unwrap();
        store.conn.execute("UPDATE tasks SET result_refs='['", []).unwrap();
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER hold_outcome BEFORE UPDATE OF status ON turn_runs
             WHEN NEW.status IN ('COMPLETED','FAILED')
             BEGIN SELECT RAISE(ABORT, 'hold returned outcome'); END;",
            )
            .unwrap();
        if failed {
            request.respond(503, json!({"error":{"message":"ORIGINAL_MODEL_FAILURE"}}));
        } else {
            request.reply(json!({"role":"assistant","content":"ORIGINAL_FINAL_REPLY"}));
        }
        let path = checkpoint(&env, "returned-outcome", &run_id);
        assert!(support::wait_for(
            || {
                std::fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Json>(&bytes).ok())
                    .is_some_and(|saved| saved["outcome"]["status"] == if failed { "FAILED" } else { "COMPLETED" })
            },
            5_000
        ));
        model.assert_quiet();
        if shutdown.starts_with("cancel-finalize") {
            store.conn.execute("UPDATE tasks SET result_refs='[]'", []).unwrap();
            wait_state(&mut worker, |state| {
                state["runtime_errors"].as_array().unwrap().iter().any(|error| error["phase"] == "finalize")
            });
        }
        if shutdown.starts_with("cancel") {
            let receipt = worker
                .call(
                    "submit",
                    json!({"action":{"action_id":"cancel-returned","kind":"cancel_run","payload":{"run_id":run_id}}}),
                )
                .unwrap();
            assert_eq!(receipt["ok"], true, "{receipt}");
            assert!(store
                .conn
                .query_row("SELECT cancel_requested FROM turn_runs", [], |row| row.get::<_, bool>(0))
                .unwrap());
        }
        if shutdown == "close" || shutdown.ends_with("crash") {
            if shutdown.ends_with("crash") {
                worker.crash();
                drop(worker);
            } else {
                worker.close();
            }
            assert_eq!(
                store
                    .conn
                    .query_row("SELECT status FROM turn_runs WHERE run_id=?1", [&run_id], |row| row.get::<_, String>(0))
                    .unwrap(),
                "RUNNING",
                "the original outcome is still waiting on durable state"
            );
            store.conn.execute("UPDATE tasks SET result_refs='[]'", []).unwrap();
            store.conn.execute_batch("DROP TRIGGER hold_outcome").unwrap();
            if shutdown.contains("legacy") {
                // An older build requeued completed checkpoints before
                // archiving them. Simulate its durable intermediate state.
                store.conn.execute("UPDATE turn_runs SET status='QUEUED' WHERE run_id=?1", [&run_id]).unwrap();
            }
            if shutdown.contains("admission") {
                store
                    .conn
                    .execute_batch(
                        "CREATE TRIGGER hold_recovery BEFORE UPDATE OF status ON turn_runs
                     WHEN NEW.status='RUNNING'
                     BEGIN SELECT RAISE(ABORT, 'hold queued recovery'); END;",
                    )
                    .unwrap();
            }
            worker = open_chat_with_mode(&env, "returned-outcome", shutdown.contains("auto"));
            if shutdown.contains("admission") {
                let waiting = wait_state(&mut worker, |state| {
                    state["runtime_errors"].as_array().unwrap().iter().any(|error| error["phase"] == "reconcile")
                });
                assert_eq!(waiting["runs"][0]["status"], "QUEUED");
                model.assert_quiet();
                assert!(worker.call("user_message", json!({"text":"UNACCEPTED_DURING_RECOVERY"})).is_err());
                store.conn.execute_batch("DROP TRIGGER hold_recovery").unwrap();
            }
        } else {
            store.conn.execute("UPDATE tasks SET result_refs='[]'", []).unwrap();
            store.conn.execute_batch("DROP TRIGGER hold_outcome").unwrap();
        }
        let state = wait_state(&mut worker, |state| {
            matches!(
                state["runs"][0]["status"].as_str(),
                Some("COMPLETED" | "FAILED" | "CANCELLED" | "OUTCOME_UNKNOWN")
            )
        });
        assert_eq!(state["runs"].as_array().unwrap().len(), 1);
        assert_eq!(state["runs"][0]["status"], if failed { "FAILED" } else { "COMPLETED" });
        assert_eq!(state["runs"][0]["run_id"], run_id);
        if shutdown.contains("auto") {
            assert_eq!(state["session"]["permissions_mode"], "full_auto");
        }
        let events = state["events"].as_array().unwrap();
        if failed {
            let failures: Vec<_> = events.iter().filter(|event| event["kind"] == "run_failed").collect();
            assert_eq!(failures.len(), 1);
            assert!(failures[0]["payload"]["error"].as_str().unwrap().contains("ORIGINAL_MODEL_FAILURE"));
        } else {
            let replies: Vec<_> = events.iter().filter(|event| event["kind"] == "leader_reply").collect();
            assert_eq!(replies.len(), 1);
            assert_eq!(replies[0]["payload"]["text"], "ORIGINAL_FINAL_REPLY");
            assert!(!events.iter().any(|event| event["kind"] == "run_failed"));
        }
        assert_eq!(std::fs::read_to_string(env.join("project/effect.txt")).unwrap(), "written once\n");
        model.assert_quiet();
        worker.close();
    }
}

#[test]
fn unstarted_member_with_an_unreadable_view_can_be_cancelled_without_a_model_request() {
    for mode in ["active", "paused", "recovered"] {
        let model = Model::start();
        let env = chat_fixture(&model, "cancel-before-prepare");
        let mut worker = open_chat(&env, "cancel-before-prepare");
        let mut spec = worker.state()["spec"].clone();
        spec["shared_spaces"] = json!([{"id":"main","readers":["leader"],"writers":["leader"]}]);
        worker.call("call", json!({"method":"save_spec","params":{"spec":spec}})).unwrap();
        let db =
            rusqlite::Connection::open(env.join("state/teamagents/sessions/cancel-before-prepare/team.db")).unwrap();
        db.execute("INSERT INTO shared_cursors VALUES('cancel-before-prepare','leader','main','broken')", []).unwrap();
        worker.call("user_message", json!({"text":"CANCEL_BEFORE_FIRST_REQUEST"})).unwrap();
        let state = wait_state(&mut worker, |state| {
            state["runtime_errors"].as_array().unwrap().iter().any(|error| error["phase"] == "prepare")
        });
        let run_id = state["runs"][0]["run_id"].clone();
        if mode == "paused" {
            let receipt = worker
                .call("submit", json!({"action":{"action_id":"pause","kind":"pause_session","payload":{}}}))
                .unwrap();
            assert_eq!(receipt["ok"], true);
        }
        if mode == "recovered" {
            worker.crash();
            drop(worker);
            db.execute("UPDATE turn_runs SET cancel_requested=1 WHERE run_id=?1", [run_id.as_str().unwrap()]).unwrap();
            worker = open_chat(&env, "cancel-before-prepare");
        } else {
            let receipt = worker
                .call(
                    "submit",
                    json!({"action":{"action_id":"cancel","kind":"cancel_run","payload":{"run_id":run_id}}}),
                )
                .unwrap();
            assert_eq!(receipt["ok"], true, "{receipt}");
        }
        let state = wait_state(&mut worker, |state| state["runs"][0]["status"] == "CANCELLED");
        assert_eq!(state["runs"][0]["status"], "CANCELLED");
        assert_eq!(state["runs"].as_array().unwrap().len(), 1);
        wait_state(&mut worker, |state| state["runtime_errors"].as_array().unwrap().is_empty());
        assert_eq!(
            state["events"].as_array().unwrap().iter().filter(|event| event["kind"] == "run_started").count(),
            0
        );
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM deliveries WHERE status='pending'", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        model.assert_quiet();
        db.execute("UPDATE shared_cursors SET sequence=0", []).unwrap();
        worker.close();
        let mut resumed = open_chat(&env, "cancel-before-prepare");
        model.assert_quiet();
        resumed.call("user_message", json!({"text":"NEW_REQUEST_AFTER_CANCEL"})).unwrap();
        let request = model.next();
        assert_eq!(request.user_messages_containing("CANCEL_BEFORE_FIRST_REQUEST"), 0);
        assert_eq!(request.user_messages_containing("NEW_REQUEST_AFTER_CANCEL"), 1);
        request.reply(json!({"role":"assistant","content":"NEW_REQUEST_COMPLETED"}));
        let state = wait_state(&mut resumed, |state| {
            state["runs"].as_array().unwrap().iter().any(|run| run["status"] == "COMPLETED")
        });
        assert_eq!(state["runs"].as_array().unwrap().len(), 2);
        assert!(state["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|run| run["run_id"] == run_id && run["status"] == "CANCELLED"));
        model.assert_quiet();
        resumed.close();
    }
}

#[test]
fn unreadable_member_view_keeps_one_queued_run_and_resumes_the_original_input() {
    let model = Model::start();
    let env = chat_fixture(&model, "view-preparation-retry");
    let mut worker = open_chat(&env, "view-retry");
    let mut spec = worker.state()["spec"].clone();
    spec["shared_spaces"] = json!([{"id":"main","readers":["leader"],"writers":["leader"]}]);
    worker.call("call", json!({"method":"save_spec","params":{"spec":spec}})).unwrap();
    let db = rusqlite::Connection::open(env.join("state/teamagents/sessions/view-retry/team.db")).unwrap();
    db.execute("INSERT INTO shared_cursors VALUES('view-retry','leader','main','broken')", []).unwrap();
    let receipt = worker.call("user_message", json!({"text":"ORIGINAL_UNCONSUMED_INPUT"})).unwrap();
    assert_eq!(receipt["ok"], true);
    let multiplied = support::wait_for(
        || db.query_row("SELECT COUNT(*) FROM turn_runs", [], |row| row.get::<_, i64>(0)).unwrap() > 1,
        1200,
    );
    assert!(!multiplied, "preparation failure repeatedly spent the goal's turn budget");
    model.assert_quiet();
    let state = worker.state();
    assert_eq!(state["runs"].as_array().unwrap().len(), 1);
    assert_eq!(state["runs"][0]["status"], "QUEUED");
    assert!(!state["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| { event["kind"] == "run_started" || event["kind"] == "run_failed" }));
    let errors = state["runtime_errors"].as_array().expect("the waiting reason must be visible to the UI");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["phase"], "prepare");
    let run_id = state["runs"][0]["run_id"].clone();
    assert_eq!(errors[0]["run_id"], run_id);
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM deliveries WHERE status='pending'", [], |row| row.get::<_, i64>(0)).unwrap(),
        1
    );
    db.execute("UPDATE shared_cursors SET sequence=0", []).unwrap();
    let request = model.next();
    assert_eq!(request.user_messages_containing("ORIGINAL_UNCONSUMED_INPUT"), 1);
    request.reply(json!({"role":"assistant","content":"VIEW_RECOVERED"}));
    let state = wait_state(&mut worker, |state| state["runs"][0]["status"] == "COMPLETED");
    assert_eq!(state["runs"].as_array().unwrap().len(), 1);
    assert_eq!(state["runs"][0]["run_id"], run_id);
    assert!(state["runtime_errors"].as_array().unwrap().is_empty());
    assert_eq!(state["events"].as_array().unwrap().iter().filter(|event| event["kind"] == "run_started").count(), 1);
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM deliveries WHERE status='pending'", [], |row| row.get::<_, i64>(0)).unwrap(),
        0
    );
    model.assert_quiet();
    worker.close();
}

#[test]
fn queued_chat_with_a_lost_checkpoint_uses_start_evidence_instead_of_replaying() {
    if !teamagents_engine::tools::bwrap_available() {
        eprintln!("skipped: bwrap is unavailable");
        return;
    }
    for mode in ["resume", "cancel", "prune", "auto-prune"] {
        let model = Model::start();
        let env = chat_fixture(&model, "queued-lost-checkpoint");
        let mut worker = open_chat(&env, "lost-checkpoint");
        worker.call("user_message", json!({"text":"perform the external write once"})).unwrap();
        model.next().reply(shell_message("write-once", "printf 'once\\n' >> effect.txt", false));
        let pending = model.next();
        assert_eq!(pending.tools("write-once").len(), 1);
        assert_eq!(std::fs::read_to_string(env.join("project/effect.txt")).unwrap(), "once\n");
        let state = worker.state();
        let run_id = state["runs"][0]["run_id"].as_str().unwrap().to_string();
        assert!(state["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| { event["kind"] == "run_started" && event["payload"]["run_id"] == run_id }));
        let path = checkpoint(&env, "lost-checkpoint", &run_id);
        assert!(path.exists());
        worker.crash();
        drop(worker);
        drop(pending);
        std::fs::remove_file(&path).unwrap();
        let db = teamagents_core::storage::Store::open(&env.join("state/teamagents/sessions/lost-checkpoint/team.db"))
            .unwrap();
        db.conn
            .execute(
                "UPDATE turn_runs SET status='QUEUED',cancel_requested=?1 WHERE run_id=?2",
                rusqlite::params![mode == "cancel", run_id],
            )
            .unwrap();
        if matches!(mode, "prune" | "auto-prune") {
            db.conn
                .execute(
                    "UPDATE events SET created_at=?1 WHERE kind='run_started'",
                    [teamagents_core::models::now() - 40.0 * 86_400.0],
                )
                .unwrap();
            if mode == "prune" {
                db.prune_history("lost-checkpoint", 30, false).unwrap();
            } else {
                let config = env.join("config/teamagents/config.toml");
                let mut text = std::fs::read_to_string(&config).unwrap();
                text.push_str("\n[retention]\nhistory_days=30\n");
                std::fs::write(config, text).unwrap();
            }
        }
        let mut resumed = open_chat(&env, "lost-checkpoint");
        model.assert_quiet();
        let state = wait_state(&mut resumed, |state| {
            matches!(state["runs"][0]["status"].as_str(), Some("OUTCOME_UNKNOWN" | "CANCELLED"))
        });
        assert_eq!(state["runs"][0]["status"], "OUTCOME_UNKNOWN", "{mode}");
        assert_eq!(state["runs"].as_array().unwrap().len(), 1);
        assert_eq!(state["runs"][0]["run_id"], run_id);
        assert_eq!(std::fs::read_to_string(env.join("project/effect.txt")).unwrap(), "once\n");
        assert!(!path.exists(), "recovery must not invent a replacement checkpoint");
        assert!(state["events"].as_array().unwrap().iter().any(|event| {
            event["kind"] == "run_failed"
                && event["payload"]["error"].as_str().is_some_and(|error| error.contains("checkpoint"))
        }));
        resumed.close();
    }
}

#[test]
fn queued_chat_recovery_distinguishes_unstarted_and_invalid_checkpoints() {
    for invalid in [false, true] {
        let model = Model::start();
        let env = chat_fixture(&model, "queued-checkpoint-evidence");
        let mut worker = open_chat(&env, "queued-evidence");
        let db = rusqlite::Connection::open(env.join("state/teamagents/sessions/queued-evidence/team.db")).unwrap();
        db.execute_batch(
            "CREATE TRIGGER hold_start BEFORE UPDATE OF status ON turn_runs
             WHEN NEW.status='RUNNING'
             BEGIN SELECT RAISE(ABORT, 'hold first execution'); END;",
        )
        .unwrap();
        worker.call("user_message", json!({"text":"ORIGINAL_QUEUED_INPUT"})).unwrap();
        let state = wait_state(&mut worker, |state| {
            state["runtime_errors"].as_array().unwrap().iter().any(|error| error["phase"] == "prepare")
        });
        let run_id = state["runs"][0]["run_id"].as_str().unwrap().to_string();
        let path = checkpoint(&env, "queued-evidence", &run_id);
        assert_eq!(state["runs"][0]["status"], "QUEUED");
        assert!(!path.exists());
        model.assert_quiet();
        worker.crash();
        drop(worker);
        db.execute_batch("DROP TRIGGER hold_start").unwrap();
        if invalid {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "{invalid checkpoint").unwrap();
            db.execute("UPDATE turn_runs SET cancel_requested=1", []).unwrap();
        }
        let mut resumed = open_chat(&env, "queued-evidence");
        if !invalid {
            let request = model.next();
            assert_eq!(request.user_messages_containing("ORIGINAL_QUEUED_INPUT"), 1);
            request.reply(json!({"role":"assistant","content":"FIRST_EXECUTION_COMPLETED"}));
        }
        let state = wait_state(&mut resumed, |state| {
            matches!(state["runs"][0]["status"].as_str(), Some("COMPLETED" | "OUTCOME_UNKNOWN" | "CANCELLED"))
        });
        assert_eq!(state["runs"].as_array().unwrap().len(), 1);
        assert_eq!(state["runs"][0]["run_id"], run_id);
        assert_eq!(state["runs"][0]["status"], if invalid { "OUTCOME_UNKNOWN" } else { "COMPLETED" });
        if invalid {
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "{invalid checkpoint");
        }
        model.assert_quiet();
        resumed.close();
    }
}

#[test]
fn queued_recovery_admits_all_saved_runs_before_any_result_can_schedule_peers() {
    for before_start_input in [false, true] {
        check_queued_recovery_admission(before_start_input);
    }
}

fn check_queued_recovery_admission(before_start_input: bool) {
    use teamagents_core::control::TurnOutcome;
    use teamagents_core::models::{TurnRun, TurnStatus};
    use teamagents_engine::core_client::CoreClient;
    use teamagents_engine::gateway::ToolGateway;
    use teamagents_engine::runtime::AgentRunner;

    struct CompletedMember;
    impl AgentRunner for CompletedMember {
        fn start_or_resume(&self, _: &TurnRun, _: &Json, _: &ToolGateway, _: &Json) -> TurnOutcome {
            panic!("a saved result must not execute the member");
        }
        fn request_interrupt(&self, _: &str) -> TurnStatus {
            TurnStatus::Completed
        }
        fn query_state(&self, _: &str) -> Option<TurnStatus> {
            None
        }
        fn deliver_mid_turn(&self, _: &str, _: Vec<Json>) {}
        fn has_recovery_state(&self, _: &TurnRun) -> Result<bool, String> {
            Ok(true)
        }
        fn reconcile(&self, run: &TurnRun, gateway: &ToolGateway) -> Option<TurnOutcome> {
            // This receipt schedules other members before finalization, as a
            // recovered Codex completion does. Every old run must be protected.
            let receipt = gateway.call(
                "complete_task",
                &json!({"task_id":run.task_id,"summary":"saved completion"}),
                "recovered-complete",
            );
            assert!(receipt.ok, "{receipt:?}");
            Some(TurnOutcome { status: TurnStatus::Completed, error: None, note: None, reply_text: None })
        }
    }

    let env = support::isolated_state_home("queued-peer-recovery");
    let path = env.join("core.db");
    let core = CoreClient::open(path.to_str().unwrap(), "s").unwrap();
    core.call_in_session("create_session", json!({"cwd":env.to_str().unwrap()})).unwrap();
    core.call_in_session(
        "save_spec",
        json!({"spec":{
            "leader_id":"leader",
            "agents":[support::member("leader","leader"),support::member("b","worker"),support::member("c","worker")],
            "channels":[support::task_channel("leader", &["b","c"])]
        }}),
    )
    .unwrap();
    for member in ["b", "c"] {
        assert!(
            support::submit(
                &core,
                member,
                "leader",
                "assign_task",
                json!({
                    "assignee":member,"description":"recover saved work"
                })
            )
            .ok
        );
    }
    let old_runs: Vec<TurnRun> = serde_json::from_value(core.state().unwrap()["runs"].clone()).unwrap();
    assert_eq!(old_runs.len(), 2);
    for run in &old_runs {
        core.call_in_session("begin_run", json!({"run_id":run.run_id})).unwrap();
    }
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute("UPDATE turn_runs SET status='QUEUED',cancel_requested=1", []).unwrap();
    let harness = support::harness_with(
        core.clone(),
        vec![
            ("leader", support::scripted("leader", &json!([["end"]]), support::barriers())),
            ("b", Arc::new(CompletedMember)),
            ("c", Arc::new(CompletedMember)),
        ],
    );
    let _close = CloseRuntime(harness.runtime.clone());
    if before_start_input {
        // exec --resume submits the next input before starting its runtime.
        assert!(harness.runtime.user_message("next user input", false).unwrap().ok);
    }
    harness.runtime.start();
    let state = core.state().unwrap();
    for old in old_runs {
        let run = state["runs"].as_array().unwrap().iter().find(|run| run["run_id"] == old.run_id).unwrap();
        assert_eq!(run["status"], "COMPLETED");
    }
    assert!(state["tasks"].as_array().unwrap().iter().all(|task| task["status"] == "SUCCEEDED"));
    assert!(!state["events"].as_array().unwrap().iter().any(|event| event["kind"] == "run_cancelled"));
}

#[test]
fn mid_turn_input_retries_after_a_state_read_failure_without_another_user_action() {
    check_mid_turn_storage_recovery(true);
}

#[test]
fn mid_turn_input_retries_after_a_projection_failure_without_another_user_action() {
    check_mid_turn_storage_recovery(false);
}

#[test]
fn member_tool_messages_reach_a_running_peer_before_the_sender_finishes() {
    let model = Model::start();
    let env = chat_fixture(&model, "mid-turn-tool-message");
    std::fs::write(env.join("project/seed.txt"), "unchanged input\n").unwrap();
    let mut worker = open_chat(&env, "tool-message");
    let mut spec = worker.state()["spec"].clone();
    spec["agents"].as_array_mut().unwrap().push(json!({
        "id":"b","name":"Worker","role":"worker","runtime_kind":"deepagents","model_profile":"m",
        "tool_bindings":["files"]
    }));
    spec["channels"] = json!([
        {"source":"leader","targets":["b"],"mode":"task"},
        {"source":"b","targets":["leader"],"mode":"message"}
    ]);
    worker.call("call", json!({"method":"save_spec","params":{"spec":spec}})).unwrap();
    worker.call("user_message", json!({"text":"LEADER_MID_TURN_REQUEST"})).unwrap();
    model.next().reply(json!({"role":"assistant","content":null,"tool_calls":[{
        "id":"delegate","type":"function","function":{"name":"assign_task",
            "arguments":"{\"assignee\":\"b\",\"description\":\"WORKER_TASK_MARKER\"}"}
    }]}));
    let one = model.next();
    let two = model.next();
    let (leader, member) =
        if one.user_messages_containing("LEADER_MID_TURN_REQUEST") == 1 { (one, two) } else { (two, one) };
    assert_eq!(leader.tools("delegate").len(), 1);
    assert_eq!(member.user_messages_containing("WORKER_TASK_MARKER"), 1);
    member.reply(json!({"role":"assistant","content":null,"tool_calls":[{
        "id":"member-message","type":"function","function":{"name":"send_message",
            "arguments":"{\"target\":\"leader\",\"text\":\"MESSAGE_BEFORE_MEMBER_FINISHES\"}"}
    }]}));
    let member_waiting = model.next();
    let receipt: Json =
        serde_json::from_str(member_waiting.tools("member-message")[0]["content"].as_str().unwrap()).unwrap();
    assert_eq!(receipt["delivered_to"], json!(["leader"]), "{receipt}");
    // The sender is still waiting on its next response. Only scheduler polling
    // can hand off this tool-originated message; no user submit or finalization.
    model.assert_quiet();
    let state = worker.state();
    assert_eq!(state["runs"].as_array().unwrap().iter().filter(|run| run["status"] == "RUNNING").count(), 2);
    leader.reply(json!({"role":"assistant","content":null,"tool_calls":[{
        "id":"leader-read","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"seed.txt\"}"}
    }]}));
    let leader_waiting = model.next();
    assert_eq!(leader_waiting.user_messages_containing("MESSAGE_BEFORE_MEMBER_FINISHES"), 1);
    assert_eq!(leader_waiting.tools("leader-read").len(), 1);
    model.assert_quiet();
    leader_waiting.reply(json!({"role":"assistant","content":null,"tool_calls":[{
        "id":"leader-read-again","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"seed.txt\"}"}
    }]}));
    let repeated = model.next();
    assert_eq!(repeated.user_messages_containing("MESSAGE_BEFORE_MEMBER_FINISHES"), 1);
    assert_eq!(std::fs::read_to_string(env.join("project/seed.txt")).unwrap(), "unchanged input\n");
    worker.close();
    drop((member_waiting, repeated));
}

#[test]
fn chat_cold_recovery_retries_requeue_failure_without_replaying_tools() {
    let model = Model::start();
    let env = chat_fixture(&model, "requeue-storage-retry");
    let mut worker = open_chat(&env, "requeue-retry");
    worker.call("user_message", json!({"text":"ORIGINAL_REQUEUE_REQUEST"})).unwrap();
    model.next().reply(json!({"role":"assistant","content":null,"tool_calls":[{
        "id":"before-crash","type":"function","function":{"name":"write_file",
            "arguments":"{\"path\":\"effect.txt\",\"content\":\"original effect\\n\"}"}
    }]}));
    let interrupted = model.next();
    let original_receipt = interrupted.tools("before-crash")[0].clone();
    assert_eq!(std::fs::read_to_string(env.join("project/effect.txt")).unwrap(), "original effect\n");
    let run_id = worker.state()["runs"][0]["run_id"].clone();
    worker.crash();
    drop((interrupted, worker));
    std::fs::write(env.join("project/effect.txt"), "USER_EDIT_AFTER_CRASH\n").unwrap();
    let db = rusqlite::Connection::open(env.join("state/teamagents/sessions/requeue-retry/team.db")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER refuse_requeue BEFORE UPDATE OF status ON turn_runs
         WHEN OLD.status='RUNNING' AND NEW.status='QUEUED'
         BEGIN SELECT RAISE(ABORT, 'requeue storage unavailable'); END;",
    )
    .unwrap();
    let mut resumed = open_chat(&env, "requeue-retry");
    let state = resumed.state();
    assert_eq!(state["runs"].as_array().unwrap().len(), 1);
    assert_eq!(state["runs"][0]["status"], "RUNNING");
    assert!(state["runtime_errors"].as_array().unwrap().iter().any(|error| {
        error["phase"] == "reconcile"
            && error["error"].as_str().is_some_and(|text| text.contains("requeue storage unavailable"))
    }));
    model.assert_quiet();
    db.execute_batch("DROP TRIGGER refuse_requeue").unwrap();
    let request = model.next();
    assert_eq!(request.tools("before-crash"), vec![&original_receipt]);
    assert_eq!(request.user_messages_containing("ORIGINAL_REQUEUE_REQUEST"), 1);
    request.reply(json!({"role":"assistant","content":"REQUEUE_RECOVERED"}));
    let state = wait_state(&mut resumed, |state| state["runs"][0]["status"] == "COMPLETED");
    assert_eq!(state["runs"].as_array().unwrap().len(), 1);
    assert_eq!(state["runs"][0]["run_id"], run_id);
    assert!(state["runtime_errors"].as_array().unwrap().is_empty());
    assert_eq!(std::fs::read_to_string(env.join("project/effect.txt")).unwrap(), "USER_EDIT_AFTER_CRASH\n");
    model.assert_quiet();
    resumed.close();
}

fn check_mid_turn_storage_recovery(corrupt_state: bool) {
    use teamagents_core::models::Task;
    use teamagents_core::storage::Store;

    let model = Model::start();
    let env = chat_fixture(&model, "mid-turn-storage");
    std::fs::write(env.join("project/seed.txt"), "unchanged input\n").unwrap();
    let mut worker = open_chat(&env, "mid-turn-storage");
    worker.call("user_message", json!({"text":"ORIGINAL_MID_TURN_REQUEST"})).unwrap();
    let first = model.next();
    let run_id = worker.state()["runs"][0]["run_id"].clone();
    let store = Store::open(&env.join("state/teamagents/sessions/mid-turn-storage/team.db")).unwrap();
    if corrupt_state {
        let task: Task = serde_json::from_value(json!({
            "task_id":"old-task","requester":"leader","assignee":"leader","description":"prior work","status":"SUCCEEDED"
        })).unwrap();
        store.insert_task("mid-turn-storage", &task).unwrap();
        store.conn.execute("UPDATE tasks SET result_refs='['", []).unwrap();
    } else {
        // Fail projection only after the input and its successful receipt have
        // committed. The authoritative state snapshot remains readable.
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER unreadable_delivery AFTER INSERT ON actions
                 WHEN NEW.kind='user_supplement'
                 BEGIN
                   UPDATE deliveries SET payload_override='99'
                   WHERE event_id IN (SELECT event_id FROM events WHERE causation_id=NEW.action_id);
                 END;",
            )
            .unwrap();
    }
    let receipt =
        worker.call("user_message", json!({"text":"ACCEPTED_DURING_STORAGE_FAULT","supplement":true})).unwrap();
    assert_eq!(receipt["ok"], true, "the supplement must have committed: {receipt}");
    assert_eq!(
        store
            .conn
            .query_row("SELECT COUNT(*) FROM deliveries WHERE status='pending'", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        2,
        "neither the held request nor the supplement is acknowledged yet"
    );
    if corrupt_state {
        let error = worker.call("call", json!({"method":"state","params":{}})).unwrap_err();
        assert!(error.contains("result_refs"), "{error}");
        store.conn.execute("UPDATE tasks SET result_refs='[]'", []).unwrap();
    } else {
        assert_eq!(
            store
                .conn
                .query_row("SELECT COUNT(*) FROM deliveries WHERE payload_override='99'", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        let state = worker.state();
        assert!(
            state["runtime_errors"].as_array().unwrap().iter().any(|error| {
                error["phase"] == "delivery"
                    && error["error"].as_str().is_some_and(|text| text.contains("payload_override"))
            }),
            "the failed projection must be visible before retry: {state}"
        );
        store
            .conn
            .execute_batch("DROP TRIGGER unreadable_delivery; UPDATE deliveries SET payload_override=NULL;")
            .unwrap();
    }
    // Only read state while waiting for automatic recovery. A new input,
    // approval, tool action, or turn completion must not be required to retry.
    wait_state(&mut worker, |state| state["runtime_errors"].as_array().unwrap().is_empty());
    model.assert_quiet();
    first.reply(json!({"role":"assistant","content":null,"tool_calls":[{
        "id":"read-seed","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"seed.txt\"}"}
    }]}));
    let next = model.next();
    assert_eq!(
        next.user_messages_containing("ACCEPTED_DURING_STORAGE_FAULT"),
        1,
        "accepted supplement must reach the next model boundary of the original run"
    );
    assert_eq!(next.user_messages_containing("ORIGINAL_MID_TURN_REQUEST"), 1);
    assert_eq!(next.tools("read-seed").len(), 1);
    assert_eq!(worker.state()["runs"].as_array().unwrap().len(), 1);
    worker.call("user_message", json!({"text":"NEXT_ACCEPTED_SUPPLEMENT","supplement":true})).unwrap();
    next.reply(json!({"role":"assistant","content":null,"tool_calls":[{
        "id":"read-again","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"seed.txt\"}"}
    }]}));
    let final_request = model.next();
    assert_eq!(final_request.user_messages_containing("ACCEPTED_DURING_STORAGE_FAULT"), 1);
    assert_eq!(final_request.user_messages_containing("NEXT_ACCEPTED_SUPPLEMENT"), 1);
    final_request.reply(json!({"role":"assistant","content":"ALL_SUPPLEMENTS_RECEIVED"}));
    let state = wait_state(&mut worker, |state| state["runs"][0]["status"] == "COMPLETED");
    assert_eq!(state["runs"].as_array().unwrap().len(), 1);
    assert_eq!(state["runs"][0]["run_id"], run_id);
    assert!(state["runtime_errors"].as_array().unwrap().is_empty());
    assert_eq!(
        store
            .conn
            .query_row("SELECT COUNT(*) FROM deliveries WHERE status='pending'", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(std::fs::read_to_string(env.join("project/seed.txt")).unwrap(), "unchanged input\n");
    model.assert_quiet();
    worker.close();
}

#[test]
fn chat_failed_outcome_commit_is_retried_without_restarting_the_member() {
    if !teamagents_engine::tools::bwrap_available() {
        eprintln!("skipped: bwrap is unavailable");
        return;
    }
    let model = Model::start();
    let env = chat_fixture(&model, "chat-finalization-retry");
    let mut worker = open_chat(&env, "commit-failure");
    worker.call("user_message", json!({"text":"perform one command before a model error"})).unwrap();
    model.next().reply(shell_message("durable-effect", "printf 'once\\n' >> effect.txt", false));
    let request = model.next();
    assert_eq!(request.tools("durable-effect").len(), 1);
    assert_eq!(std::fs::read_to_string(env.join("project/effect.txt")).unwrap(), "once\n");
    let run_id = worker.state()["runs"][0]["run_id"].as_str().unwrap().to_string();
    let db = rusqlite::Connection::open(env.join("state/teamagents/sessions/commit-failure/team.db")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER fail_finalization BEFORE UPDATE OF status ON turn_runs
         WHEN NEW.status = 'FAILED'
         BEGIN SELECT RAISE(FAIL, 'injected finalization commit failure'); END;",
    )
    .unwrap();
    request.respond(503, json!({"error":{"message":"fixture model unavailable"}}));
    assert!(support::wait_for(
        || std::fs::read_to_string(&worker.stderr).unwrap().contains("injected finalization commit failure"),
        5_000
    ));
    model.assert_quiet();
    db.execute_batch("DROP TRIGGER fail_finalization").unwrap();
    let state = wait_state(&mut worker, |state| state["runs"][0]["status"] == "FAILED");
    assert_eq!(state["runs"].as_array().unwrap().len(), 1);
    assert_eq!(state["runs"][0]["run_id"], run_id);
    let failed: Vec<_> = state["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["kind"] == "run_failed" && event["payload"]["run_id"] == run_id)
        .collect();
    assert_eq!(failed.len(), 1);
    assert!(failed[0]["payload"]["error"].as_str().unwrap().contains("fixture model unavailable"));
    assert_eq!(std::fs::read_to_string(env.join("project/effect.txt")).unwrap(), "once\n");
    model.assert_quiet();
    worker.close();
}

#[test]
fn chat_completed_checkpoint_survives_an_uncommitted_outcome_and_shutdown() {
    for crash in [false, true] {
        let model = Model::start();
        let env = chat_fixture(&model, "chat-completed-commit");
        let mut worker = open_chat(&env, "completed-commit");
        worker.call("user_message", json!({"text":"complete before core commit"})).unwrap();
        let request = model.next();
        let run_id = worker.state()["runs"][0]["run_id"].as_str().unwrap().to_string();
        let db = rusqlite::Connection::open(env.join("state/teamagents/sessions/completed-commit/team.db")).unwrap();
        db.execute_batch(
            "CREATE TRIGGER fail_finalization BEFORE UPDATE OF status ON turn_runs
             WHEN NEW.status = 'COMPLETED'
             BEGIN SELECT RAISE(FAIL, 'injected completion commit failure'); END;",
        )
        .unwrap();
        request.reply(json!({"role":"assistant","content":"DURABLE_FINAL_REPLY"}));
        assert!(support::wait_for(
            || std::fs::read_to_string(&worker.stderr).unwrap().contains("injected completion commit failure"),
            5_000
        ));
        let saved: Json =
            serde_json::from_slice(&std::fs::read(checkpoint(&env, "completed-commit", &run_id)).unwrap()).unwrap();
        assert_eq!(saved["outcome"]["status"], "COMPLETED");
        assert_eq!(saved["outcome"]["reply_text"], "DURABLE_FINAL_REPLY");
        let state = worker.state();
        assert_eq!(state["runs"][0]["status"], "RUNNING");
        assert!(!state["events"].to_string().contains("DURABLE_FINAL_REPLY"));
        model.assert_quiet();
        if crash {
            worker.crash();
            drop(worker);
        } else {
            worker.close();
        }
        db.execute_batch("DROP TRIGGER fail_finalization").unwrap();
        let mut resumed = open_chat(&env, "completed-commit");
        let state = wait_state(&mut resumed, |state| state["runs"][0]["status"] == "COMPLETED");
        assert_eq!(state["runs"].as_array().unwrap().len(), 1);
        assert_eq!(state["runs"][0]["run_id"], run_id);
        let replies: Vec<_> =
            state["events"].as_array().unwrap().iter().filter(|event| event["kind"] == "leader_reply").collect();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0]["payload"]["text"], "DURABLE_FINAL_REPLY");
        model.assert_quiet();
        resumed.close();
    }
}

#[test]
fn chat_model_failure_survives_a_failed_commit_and_sigkill_without_another_request() {
    let model = Model::start();
    let env = chat_fixture(&model, "chat-failed-cold-commit");
    let mut worker = open_chat(&env, "failed-cold-commit");
    worker.call("user_message", json!({"text":"preserve a model failure across restart"})).unwrap();
    let request = model.next();
    let run_id = worker.state()["runs"][0]["run_id"].as_str().unwrap().to_string();
    let db = rusqlite::Connection::open(env.join("state/teamagents/sessions/failed-cold-commit/team.db")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER fail_finalization BEFORE UPDATE OF status ON turn_runs
         WHEN NEW.status = 'FAILED'
         BEGIN SELECT RAISE(FAIL, 'injected failed outcome commit failure'); END;",
    )
    .unwrap();
    request.respond(503, json!({"error":{"message":"persistent model failure marker"}}));
    assert!(support::wait_for(
        || std::fs::read_to_string(&worker.stderr).unwrap().contains("injected failed outcome commit failure"),
        5_000
    ));
    worker.crash();
    drop(worker);
    db.execute_batch("DROP TRIGGER fail_finalization").unwrap();
    let mut resumed = open_chat(&env, "failed-cold-commit");
    model.assert_quiet();
    let state = wait_state(&mut resumed, |state| state["runs"][0]["status"] == "FAILED");
    assert_eq!(state["runs"].as_array().unwrap().len(), 1);
    assert_eq!(state["runs"][0]["run_id"], run_id);
    let failures: Vec<_> =
        state["events"].as_array().unwrap().iter().filter(|event| event["kind"] == "run_failed").collect();
    assert_eq!(failures.len(), 1);
    assert!(failures[0]["payload"]["error"].as_str().unwrap().contains("persistent model failure marker"));
    resumed.close();
}

#[test]
fn finalization_retries_while_paused_without_blocking_other_members_or_premature_hooks() {
    use teamagents_core::models::{ActionKind, TeamAction};
    use teamagents_engine::core_client::CoreClient;

    let env = support::isolated_state_home("finalization-paused");
    let path = env.join("core.db");
    let core = CoreClient::open(path.to_str().unwrap(), "s").unwrap();
    core.call_in_session("create_session", json!({"cwd":env.to_str().unwrap()})).unwrap();
    core.call_in_session(
        "save_spec",
        json!({"spec":{
            "leader_id":"leader", "agents":[support::member("leader","leader"), support::member("b","worker")]
        }}),
    )
    .unwrap();
    let leader = support::scripted("leader", &json!([["fail", "original member error"]]), support::barriers());
    let worker = support::scripted(
        "b",
        &json!([
            ["call","complete_task",{"task_id":"$run.task_id","summary":"independent work done"}],["end"]
        ]),
        support::barriers(),
    );
    let harness = support::harness_with(core.clone(), vec![("leader", leader.clone()), ("b", worker)]);
    let _close = CloseRuntime(harness.runtime.clone());
    let events = Arc::new(std::sync::Mutex::new(Vec::<(String, Json)>::new()));
    let recorded = events.clone();
    harness.runtime.notify.set_event_sink(Box::new(move |event, payload| {
        recorded.lock().unwrap().push((event.to_string(), payload.clone()));
    }));
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute_batch(
        "CREATE TRIGGER fail_finalization BEFORE UPDATE OF status ON turn_runs
         WHEN NEW.agent_id = 'leader' AND NEW.status = 'FAILED'
         BEGIN SELECT RAISE(FAIL, 'injected leader commit failure'); END;",
    )
    .unwrap();
    harness.runtime.start();
    harness.runtime.user_message("original input for failed member", false).unwrap();
    assert!(support::wait_for(|| leader.cursor.load(Ordering::SeqCst) == 1, 5_000));
    let leader_run = core.state().unwrap()["runs"][0]["run_id"].as_str().unwrap().to_string();
    let receipt = support::submit(
        &core,
        "independent-task",
        "leader",
        "assign_task",
        json!({"assignee":"b","description":"independent work"}),
    );
    assert!(receipt.ok, "{receipt:?}");
    assert!(support::wait_for(|| core.state().unwrap()["tasks"][0]["status"] == "SUCCEEDED", 5_000));
    assert!(
        !events.lock().unwrap().iter().any(|(kind, payload)| kind == "run_failed" && payload["run_id"] == leader_run),
        "failed commits must not announce finalized outcomes to hooks"
    );
    assert_eq!(leader.cursor.load(Ordering::SeqCst), 1);
    let receipt = harness
        .runtime
        .submit(TeamAction {
            action_id: "pause".into(),
            session_id: "s".into(),
            actor_id: "user".into(),
            run_id: None,
            kind: ActionKind::PauseSession,
            payload: json!({}),
        })
        .unwrap();
    assert!(receipt.ok);
    db.execute_batch("DROP TRIGGER fail_finalization").unwrap();
    assert!(support::wait_for(
        || events.lock().unwrap().iter().any(|(kind, payload)| kind == "run_failed" && payload["run_id"] == leader_run),
        5_000
    ));
    let state = core.state().unwrap();
    assert_eq!(state["session"]["status"], "PAUSED");
    assert_eq!(
        state["runs"].as_array().unwrap().iter().find(|run| run["run_id"] == leader_run).unwrap()["status"],
        "FAILED"
    );
    assert!(state["runs"].as_array().unwrap().iter().all(|run| run["status"] != "RUNNING"));
    assert!(
        state["runs"].as_array().unwrap().iter().any(|run| run["status"] == "QUEUED"),
        "new task notifications must remain queued while the session is paused"
    );
    assert_eq!(leader.cursor.load(Ordering::SeqCst), 1);
    let failures: Vec<_> = events
        .lock()
        .unwrap()
        .iter()
        .filter(|(kind, payload)| kind == "run_failed" && payload["run_id"] == leader_run)
        .cloned()
        .collect();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].1["error"], "original member error");
    let view = core.call_in_session("agent_view", json!({"agent_id":"leader"})).unwrap();
    assert!(!view["inbox_delta"].to_string().contains("original input for failed member"));
}

#[test]
fn finalization_hooks_report_a_cancellation_committed_while_parking() {
    check_finalization_hooks("cancel");
}

#[test]
fn finalization_hooks_do_not_repeat_a_superseded_outcome() {
    check_finalization_hooks("settled");
}

#[test]
fn finalization_hooks_do_not_report_an_already_resolved_approval_as_paused() {
    check_finalization_hooks("approval");
}

fn check_finalization_hooks(case: &str) {
    use std::sync::atomic::AtomicUsize;
    use teamagents_core::control::TurnOutcome;
    use teamagents_core::models::{TurnRun, TurnStatus};
    use teamagents_engine::core_client::CoreClient;
    use teamagents_engine::gateway::ToolGateway;
    use teamagents_engine::runtime::AgentRunner;

    struct ParkOnce {
        status: TurnStatus,
        calls: AtomicUsize,
    }
    impl AgentRunner for ParkOnce {
        fn start_or_resume(&self, _: &TurnRun, _: &Json, _: &ToolGateway, _: &Json) -> TurnOutcome {
            let status =
                if self.calls.fetch_add(1, Ordering::SeqCst) == 0 { self.status } else { TurnStatus::Completed };
            TurnOutcome { status, error: None, note: None, reply_text: None }
        }
        fn request_interrupt(&self, _: &str) -> TurnStatus {
            TurnStatus::Cancelled
        }
        fn query_state(&self, _: &str) -> Option<TurnStatus> {
            None
        }
        fn deliver_mid_turn(&self, _: &str, _: Vec<Json>) {}
    }

    let env = support::isolated_state_home("finalization-hooks");
    let path = env.join("core.db");
    let core = CoreClient::open(path.to_str().unwrap(), "s").unwrap();
    core.call_in_session("create_session", json!({"cwd":env.to_str().unwrap()})).unwrap();
    core.call_in_session(
        "save_spec",
        json!({"spec":{
            "leader_id":"leader", "agents":[support::member("leader","leader")]
        }}),
    )
    .unwrap();
    let member = Arc::new(ParkOnce {
        status: if case == "approval" { TurnStatus::WaitingApproval } else { TurnStatus::WaitingTask },
        calls: AtomicUsize::new(0),
    });
    let harness = support::harness_with(core.clone(), vec![("leader", member.clone())]);
    let _close = CloseRuntime(harness.runtime.clone());
    let events = Arc::new(std::sync::Mutex::new(Vec::<(String, Json)>::new()));
    let recorded = events.clone();
    harness.runtime.notify.set_event_sink(Box::new(move |event, payload| {
        recorded.lock().unwrap().push((event.to_string(), payload.clone()));
    }));
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute_batch(
        "CREATE TRIGGER hold_park BEFORE UPDATE OF status ON turn_runs
         WHEN NEW.status IN ('WAITING_TASK','WAITING_APPROVAL')
         BEGIN SELECT RAISE(ABORT, 'hold parked segment'); END;",
    )
    .unwrap();
    harness.runtime.start();
    harness.runtime.user_message("park before committing", false).unwrap();
    assert!(support::wait_for(|| harness.runtime.errors().iter().any(|error| error["phase"] == "finalize"), 5_000));
    let run_id = core.state().unwrap()["runs"][0]["run_id"].as_str().unwrap().to_string();
    assert!(!events.lock().unwrap().iter().any(|(event, _)| event.starts_with("run_")));
    if case != "approval" {
        let receipt = support::submit(&core, "cancel", "user", "cancel_run", json!({"run_id":run_id}));
        assert!(receipt.ok, "{receipt:?}");
        if case == "settled" {
            let inputs = core.state().unwrap()["runs"][0]["input_delivery_ids"].clone();
            core.call_in_session("finalize_run", json!({"run_id":run_id,"status":"CANCELLED","ack_ids":inputs}))
                .unwrap();
        }
    }
    db.execute_batch("DROP TRIGGER hold_park").unwrap();
    let expected = if case == "approval" { "COMPLETED" } else { "CANCELLED" };
    assert!(support::wait_for(
        || core.state().unwrap()["runs"][0]["status"] == expected && harness.runtime.errors().is_empty(),
        5_000
    ));
    harness.runtime.close();
    let actual: Vec<_> =
        events.lock().unwrap().iter().filter(|(event, _)| event.starts_with("run_")).cloned().collect();
    let expected_events = match case {
        "cancel" => vec!["run_cancelled"],
        "approval" => vec!["run_completed"],
        _ => vec![],
    };
    assert_eq!(actual.iter().map(|(event, _)| event.as_str()).collect::<Vec<_>>(), expected_events, "{case}");
    for (_, payload) in actual {
        assert_eq!(payload["run_id"], run_id);
        assert_eq!(payload["status"], expected);
        assert!(payload["error"].is_null());
        assert!(payload["reply_text"].is_null());
    }
    assert_eq!(member.calls.load(Ordering::SeqCst), if case == "approval" { 2 } else { 1 });
}
