//! Real worker protocol with production runners, without model requests.

mod support;

use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

struct Worker {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<Json>,
    pending: HashMap<u64, Json>,
    next: u64,
}

impl Worker {
    fn spawn(home: &Path) -> Self {
        let path = std::env::join_paths(
            std::iter::once(home.join("bin"))
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())),
        )
        .unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .arg("serve")
            .env("PATH", path)
            .env("XDG_STATE_HOME", home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, replies) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(reply) = serde_json::from_str::<Json>(&line) {
                    if reply["id"].is_u64() {
                        let _ = tx.send(reply);
                    }
                }
            }
        });
        Self { child, stdin, replies, pending: HashMap::new(), next: 0 }
    }

    fn send(&mut self, method: &str, params: Json) -> u64 {
        self.next += 1;
        writeln!(self.stdin, "{}", json!({"id":self.next,"method":method,"params":params})).unwrap();
        self.stdin.flush().unwrap();
        self.next
    }

    fn receive(&mut self, id: u64) -> Result<Json, String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let reply = loop {
            if let Some(reply) = self.pending.remove(&id) {
                break reply;
            }
            let reply =
                self.replies.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("worker response");
            self.pending.insert(reply["id"].as_u64().unwrap(), reply);
        };
        match reply["error"].as_str() {
            Some(error) => Err(error.into()),
            None => Ok(reply["result"].clone()),
        }
    }

    fn call(&mut self, method: &str, params: Json) -> Result<Json, String> {
        let id = self.send(method, params);
        self.receive(id)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "{}", json!({"id":0,"method":"close"}));
        let _ = self.stdin.flush();
        let end = Instant::now() + Duration::from_secs(3);
        while self.child.try_wait().ok().flatten().is_none() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn fixture(tag: &str) -> (support::TestEnv, PathBuf) {
    let env = support::isolated_state_home(tag);
    std::fs::create_dir_all(env.join("config/teamagents")).unwrap();
    std::fs::write(
        env.join("config/teamagents/config.toml"),
        "[models.leader_main]\nprovider='local'\nmodel='test'\nbase_url='http://127.0.0.1:9/v1'\nmax_retries=0\n",
    )
    .unwrap();
    let root = env.join("project");
    std::fs::create_dir(&root).unwrap();
    assert!(Command::new("git").arg("-C").arg(&root).args(["init", "-q"]).status().unwrap().success());
    std::fs::write(root.join("file"), "user input\n").unwrap();
    (env, root)
}

fn gate(home: &Path) -> PathBuf {
    std::fs::create_dir_all(home.join("bin")).unwrap();
    let gate = home.join("gate");
    let wrapper = home.join("bin/git");
    std::fs::write(&wrapper, format!(
        "#!/bin/sh\nfor arg in \"$@\"; do\n if [ \"$arg\" = ls-files ] && [ -f '{gate}' ]; then\n  echo $$ > '{started}'\n  while [ -f '{gate}' ]; do /bin/sleep 0.02; done\n fi\ndone\nexec /usr/bin/git \"$@\"\n",
        gate=gate.display(), started=home.join("started").display())).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    gate
}

fn wait_started(home: &Path) -> u32 {
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(text) = std::fs::read_to_string(home.join("started")) {
            if let Ok(pid) = text.trim().parse() {
                return pid;
            }
        }
        assert!(Instant::now() < end, "review did not reach the Git barrier");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn review_protocol_validates_input_and_preserves_baseline_across_worker_restart() {
    let (env, root) = fixture("review-protocol");
    let mut worker = Worker::spawn(&env);
    assert!(worker.call("review", json!({"agent_id":"leader"})).unwrap_err().contains("no open session"));
    let opened = worker.call("open", json!({"cwd":root})).unwrap();
    std::fs::write(root.join("file"), "after shell or backend edit\n").unwrap();
    let report = worker.call("review", json!({"agent_id":"leader","path":"file","offset":0})).unwrap();
    assert_eq!(report["shared"], true);
    assert!(report["detail"]["lines"].to_string().contains("-user input"));
    for params in [
        json!({}),
        json!({"agent_id":"ghost"}),
        json!({"agent_id":"../leader"}),
        json!({"agent_id":"leader","path":"../secret"}),
        json!({"agent_id":"leader","path":42}),
        json!({"agent_id":"leader","offset":-1}),
        json!({"agent_id":"leader","offset":1}),
        json!({"agent_id":"leader","path":"file","offset":999999}),
        json!({"agent_id":"leader","revision":1}),
        json!({"agent_id":"leader","revision":"stale"}),
    ] {
        assert!(worker.call("review", params.clone()).is_err(), "{params}");
    }
    drop(worker);
    let mut reopened = Worker::spawn(&env);
    reopened.call("open", json!({"cwd":root,"resume":opened["session_id"]})).unwrap();
    let after = reopened.call("review", json!({"agent_id":"leader","path":"file"})).unwrap();
    assert_eq!(after["baseline_at"], report["baseline_at"]);
    assert_eq!(after["detail"], report["detail"]);
    drop(reopened);
}

#[test]
fn slow_review_does_not_block_state_cancel_or_close_and_reaps_git() {
    let (env, root) = fixture("review-responsive");
    let gate = gate(&env);
    let mut worker = Worker::spawn(&env);
    let opened = worker
        .call(
            "open",
            json!({"cwd":root,"initial_spec":{"leader_id":"leader","agents":[
        {"id":"leader","name":"Leader","role":"leader","runtime_kind":"deepagents","model_profile":"leader_main"},
        {"id":"dev","name":"Dev","role":"worker","runtime_kind":"deepagents","model_profile":"leader_main"}
    ]}}),
        )
        .unwrap();
    // Establish the real production baseline, then use only deterministic
    // runners for the long-running cancellation scenario (no model traffic).
    drop(worker);
    let mut worker = Worker::spawn(&env);
    worker
        .call(
            "open",
            json!({"cwd":root,"resume":opened["session_id"],
        "scripts":{"leader":[["sleep",30],["end"]]}}),
        )
        .unwrap();
    worker.call("user_message", json!({"text":"deterministic cancellation probe"})).unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    let run = loop {
        let state = worker.call("call", json!({"method":"state","params":{"include_events":false}})).unwrap();
        if let Some(run) =
            state["runs"].as_array().unwrap().iter().find(|r| r["agent_id"] == "leader" && r["status"] == "RUNNING")
        {
            break run["run_id"].clone();
        }
        assert!(Instant::now() < end, "scripted leader never started");
        std::thread::sleep(Duration::from_millis(10));
    };
    std::fs::write(&gate, "hold").unwrap();
    worker.send("review", json!({"agent_id":"dev"}));
    let pid = wait_started(&env);
    let start = Instant::now();
    worker.call("ping", json!({})).unwrap();
    worker.call("call", json!({"method":"state","params":{"include_events":false}})).unwrap();
    assert!(worker.call("review", json!({"agent_id":"dev"})).unwrap_err().contains("进行中"));
    let cancelled = worker
        .call("submit", json!({"action":{"action_id":"cancel-probe","kind":"cancel_run","payload":{"run_id":run}}}))
        .unwrap();
    assert_eq!(cancelled["ok"], true);
    let end = Instant::now() + Duration::from_secs(2);
    loop {
        let state = worker.call("call", json!({"method":"state","params":{"include_events":false}})).unwrap();
        if state["runs"].as_array().unwrap().iter().any(|r| r["run_id"] == run && r["status"] == "CANCELLED") {
            break;
        }
        assert!(Instant::now() < end, "cancel did not converge while review was blocked: {state}");
        std::thread::sleep(Duration::from_millis(10));
    }
    worker.call("close", json!({})).unwrap();
    drop(worker);
    assert!(start.elapsed() < Duration::from_secs(3), "review blocked control or shutdown");
    let status = std::fs::read_to_string(format!("/proc/{pid}/status"));
    assert!(status.is_err() || status.unwrap().contains("State:\tZ"), "Git wrapper leaked after close");
}

#[test]
fn a_late_review_after_archive_never_recreates_session_state() {
    let (env, root) = fixture("review-archive");
    let gate = gate(&env);
    let mut worker = Worker::spawn(&env);
    let opened = worker.call("open", json!({"cwd":root})).unwrap();
    let session = env.join("teamagents/sessions").join(opened["session_id"].as_str().unwrap());
    std::fs::write(root.join("new"), "new content\n").unwrap();
    std::fs::write(&gate, "hold").unwrap();
    let id = worker.send("review", json!({"agent_id":"leader","path":"new"}));
    wait_started(&env);
    worker.call("archive_session", json!({"session_id":opened["session_id"]})).unwrap();
    assert!(!session.exists());
    std::fs::remove_file(gate).unwrap();
    let report = worker.receive(id).unwrap();
    assert!(report["detail"]["lines"].to_string().contains("+new content"));
    assert!(!session.exists(), "late read must not recreate archived state");
    drop(worker);
}
