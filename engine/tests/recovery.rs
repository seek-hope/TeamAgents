//! T8/T21: crash windows — a killed session is reconciled on restart and
//! replayed actions stay exactly-once.

use serde_json::{json, Value as Json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};

struct Worker {
    child: Child,
    stdin: ChildStdin,
    responses: Receiver<Json>,
    next_id: u64,
}

impl Worker {
    fn spawn(state_home: &std::path::Path, config_home: &std::path::Path) -> Worker {
        let mut child = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .arg("serve")
            .env("XDG_STATE_HOME", state_home)
            .env("XDG_CONFIG_HOME", config_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
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
        Worker { child, stdin, responses: rx, next_id: 1 }
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
