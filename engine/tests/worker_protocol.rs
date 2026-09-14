//! tui-worker protocol parity: the Rust engine serves the same JSON-lines API
//! (ts/test/tui-worker.test.ts port).

use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};

struct WorkerClient {
    child: Child,
    stdin: ChildStdin,
    responses: Receiver<Json>,
    #[allow(dead_code)]
    pushes: Vec<Json>,
    next_id: u64,
}

impl WorkerClient {
    fn spawn(state_home: &std::path::Path) -> WorkerClient {
        let mut child = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .arg("serve")
            .env("XDG_STATE_HOME", state_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn engine");
        let stdout = child.stdout.take().expect("stdout");
        let stdin = child.stdin.take().expect("stdin");
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
        WorkerClient { child, stdin, responses: rx, pushes: vec![], next_id: 1 }
    }

    fn call(&mut self, method: &str, params: Json) -> Result<Json, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"id": id, "method": method, "params": params});
        writeln!(self.stdin, "{request}").expect("write");
        self.stdin.flush().expect("flush");
        loop {
            let message = self
                .responses
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("worker response");
            if message.get("id").and_then(|v| v.as_u64()) != Some(id) {
                continue;
            }
            return match message.get("error").and_then(|v| v.as_str()) {
                Some(error) => Err(error.to_string()),
                None => Ok(message.get("result").cloned().unwrap_or(Json::Null)),
            };
        }
    }

    fn close(mut self) {
        let _ = self.call("close", json!({}));
        let _ = self.child.wait();
    }
}

fn state_home(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ta-worker-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn worker_drives_a_scripted_session_end_to_end() {
    let home = state_home("e2e");
    let mut worker = WorkerClient::spawn(&home);
    let opened = worker
        .call(
            "open",
            json!({
                "cwd": "/tmp",
                "scripts": {"leader": [
                    ["call", "send_message", {"target": "leader", "text": "note to self"}],
                    ["call", "signal_done", {"summary": "shipped"}],
                    ["end"],
                ]},
            }),
        )
        .expect("open");
    let session_id = opened.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    assert!(session_id.starts_with("proj_"), "session id {session_id}");
    assert!(opened.get("catalog").map(|c| c.is_object()).unwrap_or(false));

    let receipt = worker.call("user_message", json!({"text": "build it"})).expect("user_message");
    assert_eq!(receipt.get("ok").and_then(|v| v.as_bool()), Some(true));
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let state = worker
        .call("call", json!({"method": "state", "params": {"after_sequence": 0}}))
        .expect("state");
    let kinds: Vec<String> = state
        .get("events")
        .and_then(|v| v.as_array())
        .map(|events| {
            events
                .iter()
                .filter_map(|e| e.get("kind").and_then(|v| v.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert!(kinds.contains(&"user_message".to_string()), "{kinds:?}");
    assert!(kinds.contains(&"goal_done".to_string()), "{kinds:?}");
    assert_eq!(state.pointer("/session/session_id").and_then(|v| v.as_str()), Some(session_id.as_str()));

    let sessions = worker.call("list_sessions", json!({})).expect("list_sessions");
    let listed: Vec<String> = sessions
        .get("sessions")
        .and_then(|v| v.as_array())
        .map(|rows| rows.iter().filter_map(|r| r.get("sessionId").and_then(|v| v.as_str()).map(str::to_string)).collect())
        .unwrap_or_default();
    assert!(listed.contains(&session_id), "{listed:?}");

    // switch to a new session, then archive and delete it
    let fresh = worker.call("new_session", json!({})).expect("new_session");
    let fresh_id = fresh.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    assert_ne!(fresh_id, session_id);
    let archived = worker.call("archive_session", json!({"session_id": fresh_id})).expect("archive");
    assert_eq!(archived.get("was_current").and_then(|v| v.as_bool()), Some(true));
    let gone = worker.call("delete_session", json!({"session_id": session_id})).expect("delete");
    assert_eq!(gone.get("was_current").and_then(|v| v.as_bool()), Some(false));
    worker.close();
}

#[test]
fn worker_usage_reports_per_agent_counters() {
    let home = state_home("usage");
    let mut worker = WorkerClient::spawn(&home);
    let opened = worker
        .call("open", json!({"cwd": "/tmp", "scripts": {"leader": [["end"]]}}))
        .expect("open");
    let session_id = opened.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();

    let report = worker.call("usage", json!({})).expect("usage");
    assert_eq!(report.get("session_id").and_then(|v| v.as_str()), Some(session_id.as_str()));
    let agents = report.get("agents").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    assert_eq!(agents.len(), 1, "{report}");
    let leader = &agents[0];
    assert_eq!(leader.get("agent_id").and_then(|v| v.as_str()), Some("leader"));
    assert_eq!(leader.get("model_profile").and_then(|v| v.as_str()), Some("leader_main"));
    // scripted member: no model runner, hence no usage probe and no window
    assert_eq!(leader.get("usage").cloned(), Some(Json::Null));
    assert_eq!(leader.get("context_window").cloned(), Some(Json::Null));
    worker.close();
}

#[test]
fn worker_set_model_switches_and_clears_overrides() {
    let home = state_home("model");
    let mut worker = WorkerClient::spawn(&home);
    worker
        .call("open", json!({"cwd": "/tmp", "scripts": {"leader": [["end"]]}}))
        .expect("open");

    let set = worker
        .call("set_model", json!({"agent_id": "leader", "model": "gpt-5-mini", "effort": "low"}))
        .expect("set_model");
    assert_eq!(set.get("agent_id").and_then(|v| v.as_str()), Some("leader"));
    assert_eq!(set.get("model").and_then(|v| v.as_str()), Some("gpt-5-mini"));
    assert_eq!(set.get("effort").and_then(|v| v.as_str()), Some("low"));
    assert_eq!(set.get("overridden").and_then(|v| v.as_bool()), Some(true));

    // the "model" report shows the same effective values
    let report = worker.call("model", json!({})).expect("model report");
    let agents = report.get("agents").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let leader = agents
        .iter()
        .find(|a| a.get("agent_id").and_then(|v| v.as_str()) == Some("leader"))
        .expect("leader in report");
    assert_eq!(leader.get("model").and_then(|v| v.as_str()), Some("gpt-5-mini"));
    assert_eq!(leader.get("overridden").and_then(|v| v.as_bool()), Some(true));

    // unknown member / unknown effort are clean protocol errors
    let err = worker.call("set_model", json!({"agent_id": "ghost", "model": "x"})).expect_err("ghost");
    assert!(err.contains("unknown member"), "{err}");
    let err = worker
        .call("set_model", json!({"agent_id": "leader", "model": "x", "effort": "insane"}))
        .expect_err("bad effort");
    assert!(err.contains("effort"), "{err}");

    // empty params clear the override back to the profile default
    let cleared = worker.call("set_model", json!({"agent_id": "leader"})).expect("clear");
    assert_eq!(cleared.get("overridden").and_then(|v| v.as_bool()), Some(false));
    worker.close();
}

#[test]
fn worker_reports_unknown_methods_and_missing_session() {
    let home = state_home("errors");
    let mut worker = WorkerClient::spawn(&home);
    let unknown = worker.call("nope", json!({})).expect_err("unknown method");
    assert!(unknown.contains("unknown method"), "{unknown}");
    let missing = worker
        .call("call", json!({"method": "state"}))
        .expect_err("no open session");
    assert!(missing.contains("no open session"), "{missing}");
    worker.close();
    let _ = HashMap::<String, String>::new();
}
