//! 退役（R29，2026-09-24）：本文件覆盖 v1 后端（chat/codex/session/worker/审查树）。
//! v1 入口已不可达；等价覆盖在 v2：`v2_driver`/`v2_supervisor`/`v2_daemon`/`v2_mcp`/`v2_spawn_failure`、
//! `review/eval/r2-p6`（性能）与 `review/eval/r2-p5` 的真实供应商验收。文件待随模块删除。
#![cfg(any())]
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

#[test]
fn resume_rejects_invalid_leaders_without_replacing_persisted_work() {
    use teamagents_core::control::Control;
    use teamagents_core::models::{ActionKind, TeamAction, TeamSpec};
    use teamagents_core::storage::Store;
    use teamagents_engine::sessions::session_paths;

    let (_env, root) = isolate("leader-invariants");
    let cwd = root.join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let mut valid = spec("m");
    valid["agents"].as_array_mut().unwrap().push(json!({
        "id": "b", "name": "Worker", "role": "reviewer", "runtime_kind": "deepagents", "model_profile": "m"
    }));
    let mut duplicate = valid.clone();
    duplicate["agents"][1]["role"] = json!("leader");
    let mut external = valid.clone();
    external["agents"][0]["runtime_kind"] = json!("codex");
    for (id, blob, expected) in [
        ("duplicate", duplicate.to_string(), "唯一"),
        ("external", external.to_string(), "内置"),
        ("corrupt", "{broken json".into(), "stored spec is invalid"),
    ] {
        let paths = session_paths(id);
        std::fs::create_dir_all(&paths.base).unwrap();
        let store = Store::open(&paths.db).unwrap();
        store.create_session(id, cwd.to_str().unwrap(), "approved_scope").unwrap();
        let team: TeamSpec = serde_json::from_value(valid.clone()).unwrap();
        store.save_team_spec(id, &team).unwrap();
        for agent in &team.agents {
            store.ensure_agent(id, &agent.id).unwrap();
        }
        let mut control = Control::new(store, id);
        for (action_id, actor_id, kind, payload) in [
            ("user", "user", ActionKind::UserMessage, json!({"text": "preserve this goal"})),
            ("task", "leader", ActionKind::AssignTask, json!({"assignee": "b", "description": "preserve this task"})),
        ] {
            let receipt = control
                .submit(&TeamAction {
                    action_id: action_id.into(),
                    session_id: id.into(),
                    actor_id: actor_id.into(),
                    run_id: None,
                    kind,
                    payload,
                })
                .unwrap();
            assert!(receipt.ok, "{receipt:?}");
        }
        control.store.conn.execute("UPDATE team_specs SET spec_json=?1 WHERE revision=1", [&blob]).unwrap();
        drop(control);
        let snapshot = || {
            let store = Store::open(&paths.db).unwrap();
            let mut statement =
                store.conn.prepare("SELECT revision, spec_json FROM team_specs ORDER BY revision").unwrap();
            let specs: Vec<(i64, String)> = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            json!({
                "specs": specs,
                "session": store.get_session(id).unwrap(),
                "events": store.events(id, 0, 100).unwrap(),
                "tasks": store.tasks_for_session(id, &[]).unwrap(),
                "runs": store.runs_for_session(id, &[]).unwrap(),
                "receipts": (["user", "task"].map(|action| store.get_action_receipt(action).unwrap())),
                "agents": (["leader", "b"].map(|agent| (
                    store.agent_status(id, agent).unwrap(),
                    store.agent_config_revision(id, agent).unwrap(),
                    store.agent_context_epoch(id, agent).unwrap(),
                ))),
            })
        };
        let before = snapshot();
        for initial_spec in [None, Some(valid.clone())] {
            let result = open_session(OpenOptions {
                cwd: Some(cwd.clone()),
                session_id: Some(id.into()),
                full_auto: true,
                initial_spec,
                catalog: Some(catalog()),
                scripts: Some(Default::default()),
            });
            let error = match result {
                Ok(opened) => {
                    opened.close();
                    panic!("{id}: an invalid persisted spec must stop session construction");
                }
                Err(error) => error,
            };
            assert!(error.contains(expected), "{id}: {error}");
            assert_eq!(snapshot(), before, "resuming must not repair over or mutate existing work");
            assert!(!is_session_locked(id, None), "failed resume must release the lock");
            assert!(!paths.base.join("members").exists(), "validation must run before member construction");
        }
    }
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
