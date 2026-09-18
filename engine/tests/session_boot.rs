//! Session bootstrap regressions: a failed open must not keep the session
//! locked, the lock must be visible to other processes, and a kill -9 must
//! free it (findings 5/7).

use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
mod support;
use support::{isolated_state_home, TestEnv};
use teamagents_core::models::UserConfig;
use teamagents_engine::session::{open_session, OpenOptions};
use teamagents_engine::sessions::{acquire_session_lock, is_session_locked};

fn isolate(tag: &str) -> (TestEnv, PathBuf) {
    let mut env = isolated_state_home(tag);
    let root = env.to_path_buf();
    env.set("XDG_STATE_HOME", root.join("state"));
    (env, root)
}

fn catalog() -> UserConfig {
    serde_json::from_value(json!({
        "models": {"m": {"provider": "openai", "protocol": "openai", "model": "test"}},
    }))
    .unwrap()
}

fn spec(profile: &str) -> serde_json::Value {
    json!({
        "leader_id": "leader",
        "agents": [{"id": "leader", "name": "Leader", "role": "leader", "runtime_kind": "deepagents",
                    "model_profile": profile, "tool_bindings": ["files", "shell"]}],
        "shared_spaces": [{"id": "main", "readers": ["leader"], "writers": ["leader"]}],
    })
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<serde_json::Value>,
    next_id: u64,
}

impl Worker {
    fn spawn(state: &Path, config: &Path) -> Worker {
        let mut child = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .arg("serve")
            .env("XDG_STATE_HOME", state)
            .env("XDG_CONFIG_HOME", config)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn engine worker");
        let stdout = child.stdout.take().unwrap();
        let stdin = child.stdin.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) {
                    if message.get("push").is_none() {
                        let _ = tx.send(message);
                    }
                }
            }
        });
        Worker { child, stdin, replies: rx, next_id: 1 }
    }

    fn call(&mut self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(self.stdin, "{}", json!({"id": id, "method": method, "params": params})).unwrap();
        self.stdin.flush().unwrap();
        loop {
            let message = self.replies.recv_timeout(std::time::Duration::from_secs(30)).expect("worker reply");
            if message.get("id").and_then(|v| v.as_u64()) != Some(id) {
                continue;
            }
            return match message.get("error").and_then(|v| v.as_str()) {
                Some(error) => Err(error.to_string()),
                None => Ok(message.get("result").cloned().unwrap_or(serde_json::Value::Null)),
            };
        }
    }
}

/// finding 5: the lock is released on every error exit of open_session, so a
/// session whose open failed can be opened again.
#[test]
fn failed_open_releases_the_session_lock_and_can_be_retried() {
    let (_env, root) = isolate("retry");
    let cwd = root.join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let session = "proj_boot";

    let err = open_session(OpenOptions {
        cwd: Some(cwd.clone()),
        session_id: Some(session.into()),
        full_auto: false,
        initial_spec: Some(spec("m")),
        catalog: Some(UserConfig::default()),
        scripts: None,
    })
    .err()
    .expect("a member whose model profile is unknown must fail the open");
    assert!(err.contains("unknown model profile"), "{err}");
    assert!(!is_session_locked(session, None), "a failed open must not hold the session: {err}");
    assert!(acquire_session_lock(session).is_ok(), "the session is free again");

    let opened = open_session(OpenOptions {
        cwd: Some(cwd),
        session_id: Some(session.into()),
        full_auto: false,
        initial_spec: None,
        catalog: Some(catalog()),
        scripts: None,
    })
    .expect("the same session opens once the config is fixed");
    assert!(is_session_locked(session, None), "the open session is locked");
    opened.close();
    assert!(!is_session_locked(session, None), "close releases the lock");

    // an open that failed at save_spec leaves a row without a spec; retrying
    // with a fixed spec must repair the session, not report UNIQUE forever
    let err = open_session(OpenOptions {
        cwd: Some(root.join("project2")),
        session_id: Some("proj_repair".into()),
        full_auto: false,
        initial_spec: Some(json!({"leader_id": "ghost", "agents": []})),
        catalog: Some(catalog()),
        scripts: None,
    })
    .err()
    .expect("an invalid spec must fail the open");
    assert!(err.contains("ghost"), "{err}");
    std::fs::create_dir_all(root.join("project2")).unwrap();
    let repaired = open_session(OpenOptions {
        cwd: Some(root.join("project2")),
        session_id: Some("proj_repair".into()),
        full_auto: false,
        initial_spec: Some(spec("m")),
        catalog: Some(catalog()),
        scripts: None,
    })
    .expect("the fixed spec repairs the half-created session");
    repaired.close();
    let _ = std::fs::remove_dir_all(&root);
}

/// finding 7: the lock is a kernel file lock, so another process sees it.
#[test]
fn another_process_cannot_open_a_locked_session() {
    let (_env, root) = isolate("cross-process");
    let cwd = root.join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let session = "proj_cross";
    let held = acquire_session_lock(session).expect("lock");
    assert!(is_session_locked(session, None));

    let mut worker = Worker::spawn(&root.join("state"), &root.join("config"));
    let err = worker
        .call("open", json!({"cwd": cwd.to_string_lossy(), "resume": session}))
        .expect_err("the other process must be refused");
    assert!(err.contains("already running"), "{err}");
    drop(held);

    // once released, the same worker can open it (default spec + model profile)
    std::fs::create_dir_all(root.join("config/teamagents")).unwrap();
    std::fs::write(
        root.join("config/teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nprotocol = \"openai\"\nmodel = \"test\"\n",
    )
    .unwrap();
    worker
        .call("open", json!({"cwd": cwd.to_string_lossy(), "resume": session}))
        .expect("the released lock lets the worker open");

    // kill -9: the kernel drops the lock with the process
    let _ = worker.child.kill();
    let _ = worker.child.wait();
    assert!(!is_session_locked(session, None), "a killed process must not leave the session locked");
    let _ = std::fs::remove_dir_all(&root);
}
