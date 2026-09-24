//! Behavioral checks for the pinned repository task, injected only for grading.
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

struct Worker {
    root: PathBuf,
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<Value>,
    reader: Option<std::thread::JoinHandle<()>>,
    next: u64,
}

impl Worker {
    fn new() -> Self {
        Self::start(false)
    }

    fn with_session_profile_only() -> Self {
        Self::start(true)
    }

    fn start(session_only: bool) -> Self {
        let root = std::env::temp_dir().join(format!("ta-fork-hidden-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("project")).unwrap();
        fs::create_dir_all(root.join("config/teamagents")).unwrap();
        let mut user_config =
            "[models.leader_main]\nprovider=\"openai\"\nprotocol=\"openai\"\nmodel=\"default\"\n".to_string();
        if !session_only {
            user_config.push_str(
                "[models.local]\nprovider=\"openai\"\nprotocol=\"openai\"\nmodel=\"local-default\"\ncontext_window=1000000\n",
            );
        }
        fs::write(root.join("config/teamagents/config.toml"), user_config).unwrap();
        let base = root.join("state/teamagents/sessions/source");
        fs::create_dir_all(&base).unwrap();
        fs::write(
            base.join("profiles.json"),
            json!({"local":{
                "provider":"openai","protocol":"openai","model":"local-default",
                "context_window":1000000
            }})
            .to_string(),
        )
        .unwrap();
        fs::write(root.join("project/user.txt"), "keep user data").unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .arg("serve")
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let stdin = child.stdin.take().unwrap();
        let (tx, replies) = channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
                if message.get("id").is_some() && tx.send(message).is_err() {
                    break;
                }
            }
        });
        let mut worker = Self { root, child, stdin, replies, reader: Some(reader), next: 1 };
        let opened = worker.call("open", json!({
            "cwd":worker.root.join("project"),"resume":"source","fullAuto":true,
            "initial_spec":{"leader_id":"leader","agents":[
                {"id":"leader","name":"Leader","role":"leader","runtime_kind":"deepagents","model_profile":"leader_main","tool_bindings":[]},
                {"id":"worker","name":"Worker","role":"worker","runtime_kind":"deepagents","model_profile":"local","tool_bindings":[]}
            ]},
            "scripts":{"leader":[["end"]],"worker":[["sleep",30],["end"]]}
        })).unwrap();
        assert_eq!(opened["session_id"], "source");
        worker
    }

    fn base(&self, session: &str) -> PathBuf {
        self.root.join("state/teamagents/sessions").join(session)
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next;
        self.next += 1;
        writeln!(self.stdin, "{}", json!({"id":id,"method":method,"params":params})).unwrap();
        self.stdin.flush().unwrap();
        loop {
            let response = self.replies.recv_timeout(Duration::from_secs(8)).expect("worker response deadline");
            if response["id"] != id {
                continue;
            }
            return match response.get("error").and_then(Value::as_str) {
                Some(error) => Err(error.to_string()),
                None => Ok(response["result"].clone()),
            };
        }
    }

    fn state(&mut self) -> Value {
        self.call("call", json!({"method":"state","params":{"include_events":false}})).unwrap()
    }

    fn set_epoch(&self, epoch: i64) {
        rusqlite::Connection::open(self.base("source").join("team.db"))
            .unwrap()
            .execute(
                "UPDATE agent_runtime SET context_epoch=?1 WHERE session_id='source' AND agent_id='leader'",
                [epoch],
            )
            .unwrap();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn model(report: &Value, member: &str) -> Value {
    let row = report["agents"].as_array().unwrap().iter().find(|row| row["agent_id"] == member).unwrap();
    json!({"model_profile":row["model_profile"],"model":row["model"],"effort":row["effort"]})
}

#[test]
fn fork_preserves_effective_models_and_session_profiles_after_reopen() {
    let mut w = Worker::with_session_profile_only();
    w.call("set_model", json!({"agent_id":"leader","model":"leader-chosen","effort":"low"})).unwrap();
    w.call("set_model", json!({"agent_id":"worker","model":"worker-chosen","effort":"high"})).unwrap();
    let before = w.call("model", json!({})).unwrap();
    let profiles = fs::read(w.base("source").join("profiles.json")).unwrap();
    let overrides = fs::read(w.base("source").join("model_overrides.json")).unwrap();
    let fork = w.call("fork_session", json!({})).expect("session-only profiles must survive fork");
    let id = fork["session_id"].as_str().unwrap().to_string();
    assert_ne!(id, "source");
    for member in ["leader", "worker"] {
        assert_eq!(model(&w.call("model", json!({})).unwrap(), member), model(&before, member));
    }
    w.call("open", json!({"cwd":w.root.join("project"),"resume":"source"})).unwrap();
    w.call("open", json!({"cwd":w.root.join("project"),"resume":id})).unwrap();
    for member in ["leader", "worker"] {
        assert_eq!(model(&w.call("model", json!({})).unwrap(), member), model(&before, member));
    }
    assert_eq!(fs::read(w.base("source").join("profiles.json")).unwrap(), profiles);
    assert_eq!(fs::read(w.base("source").join("model_overrides.json")).unwrap(), overrides);
}

#[test]
fn fork_rekeys_branched_tree_to_the_new_context_without_mutating_source() {
    let mut w = Worker::new();
    w.set_epoch(7);
    let tree = json!({
        "ctx:leader:1":{"nodes":[],"leaf":null},
        "ctx:leader:7":{"nodes":[
            {"id":"u","parent":null,"message":{"role":"user","content":"keep this"}},
            {"id":"left","parent":"u","message":{"role":"assistant","content":"old branch"}},
            {"id":"right","parent":"u","message":{"role":"assistant","content":"active branch"}}
        ],"leaf":"right"}
    });
    let source = w.base("source").join("members/leader/chat_tree.json");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    let original = tree.to_string();
    fs::write(&source, &original).unwrap();
    let fork = w.call("fork_session", json!({})).unwrap();
    let id = fork["session_id"].as_str().unwrap();
    let state = w.state();
    let epoch = state["agents"].as_array().unwrap().iter().find(|a| a["id"] == "leader").unwrap()["context_epoch"]
        .as_i64()
        .unwrap();
    let copied: Value =
        serde_json::from_slice(&fs::read(w.base(id).join("members/leader/chat_tree.json")).unwrap()).unwrap();
    assert_eq!(copied[format!("ctx:leader:{epoch}")], tree["ctx:leader:7"]);
    assert_eq!(fs::read_to_string(&source).unwrap(), original);
}

#[test]
fn fork_migrates_legacy_linear_history_to_the_new_context() {
    let mut w = Worker::new();
    w.set_epoch(4);
    let history = json!({"ctx:leader:4":[{"role":"user","content":"legacy requirement"},{"role":"assistant","content":"retained answer"}]});
    let source = w.base("source").join("members/leader/chat_history.json");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, history.to_string()).unwrap();
    let fork = w.call("fork_session", json!({})).unwrap();
    let id = fork["session_id"].as_str().unwrap();
    let state = w.state();
    let epoch = state["agents"].as_array().unwrap().iter().find(|a| a["id"] == "leader").unwrap()["context_epoch"]
        .as_i64()
        .unwrap();
    let copied: Value =
        serde_json::from_slice(&fs::read(w.base(id).join("members/leader/chat_history.json")).unwrap()).unwrap();
    assert_eq!(copied[format!("ctx:leader:{epoch}")], history["ctx:leader:4"]);
    assert_eq!(fs::read_to_string(&source).unwrap(), history.to_string());
}

#[test]
fn fork_leaves_team_facts_private_members_and_project_files_behind() {
    let mut w = Worker::new();
    let db = rusqlite::Connection::open(w.base("source").join("team.db")).unwrap();
    db.execute("INSERT INTO tasks(task_id,session_id,requester,assignee,description,status,created_at,updated_at) VALUES('done','source','leader','worker','old work','SUCCEEDED',1,1)",[]).unwrap();
    db.execute("INSERT INTO turn_runs(run_id,session_id,agent_id,config_revision,topology_revision,status,created_at,updated_at) VALUES('old-run','source','worker',1,1,'COMPLETED',1,1)",[]).unwrap();
    for name in ["members/worker/chat_tree.json", "members/leader/shell/state.sh"] {
        let path = w.base("source").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "PRIVATE SOURCE").unwrap();
    }
    let fork = w.call("fork_session", json!({})).unwrap();
    let id = fork["session_id"].as_str().unwrap();
    let state = w.state();
    assert!(state["tasks"].as_array().unwrap().is_empty());
    assert!(state["runs"].as_array().unwrap().is_empty());
    for name in ["members/worker/chat_tree.json", "members/leader/shell/state.sh"] {
        assert!(!w.base(id).join(name).exists());
        assert_eq!(fs::read_to_string(w.base("source").join(name)).unwrap(), "PRIVATE SOURCE");
    }
    assert_eq!(fs::read_to_string(w.root.join("project/user.txt")).unwrap(), "keep user data");
    assert_eq!(db.query_row("SELECT count(*) FROM tasks WHERE task_id='done'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
}

#[test]
fn failed_open_and_switch_keep_the_source_session_usable() {
    let mut w = Worker::new();
    assert!(w.call("switch_session", json!({"session_id":"../escape"})).is_err());
    assert_eq!(w.state()["session"]["session_id"], "source");
    assert!(w
        .call("open", json!({"cwd":w.root.join("different"),"initial_spec":{"invalid":true},"fullAuto":false}))
        .is_err());
    assert_eq!(w.state()["session"]["session_id"], "source");
    assert_eq!(w.call("call", json!({"method":"session_mode","params":{}})).unwrap()["mode"], "full_auto");
    let fork = w.call("fork_session", json!({})).unwrap();
    assert_eq!(fork["forked_from"], "source");
    assert_eq!(w.state()["session"]["cwd"], json!(w.root.join("project")));
}

#[test]
fn failed_fork_cleans_its_target_and_keeps_the_source_open() {
    let mut w = Worker::with_session_profile_only();
    fs::write(w.base("source").join("profiles.json"), "{}").unwrap();
    let sessions = w.base("source").parent().unwrap().to_path_buf();
    let before: std::collections::BTreeSet<_> =
        fs::read_dir(&sessions).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert!(
        w.call("fork_session", json!({})).is_err(),
        "missing local profile must make the destination fail validation"
    );
    assert_eq!(w.state()["session"]["session_id"], "source");
    let after: std::collections::BTreeSet<_> =
        fs::read_dir(sessions).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(after, before, "failed fork must not leave a partial session");
    assert_eq!(fs::read_to_string(w.base("source").join("profiles.json")).unwrap(), "{}");
    assert_eq!(model(&w.call("model", json!({})).unwrap(), "worker")["model"], "local-default");
}

#[test]
fn fork_refuses_an_active_nonleader_without_cancelling_it() {
    let mut w = Worker::new();
    let receipt = w
        .call(
            "submit",
            json!({"action":{
                "action_id":"assign-hidden","actor_id":"leader","kind":"assign_task",
                "payload":{"assignee":"worker","description":"keep running"}
            }}),
        )
        .unwrap();
    assert_eq!(receipt["ok"], true, "{receipt}");
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if w.state()["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|run| run["agent_id"] == "worker" && run["status"] == "RUNNING")
        {
            break;
        }
        assert!(Instant::now() < deadline, "worker never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(w.call("fork_session", json!({})).is_err());
    assert_eq!(w.state()["session"]["session_id"], "source");
    assert!(w.state()["runs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|run| run["agent_id"] == "worker" && run["status"] == "RUNNING"));
}
