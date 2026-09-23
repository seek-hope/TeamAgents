//! The runner process loop (§6.2): serves one job directory over a Unix
//! socket. Entry point is the hidden `teamagents jobs-runner <dir>` command;
//! the driver never links this loop in-process — recovery depends on the
//! runner being a separate, parent-independent process.

use super::{atomic_json, boot_id, now_ms, signal_group, start_ticks, JobSpec, Journal};
use serde_json::{json, Value as Json};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

/// Test-only fault injection is compiled in but disabled unless the parent
/// explicitly opts in per runner process (never from the job file or model).
const TEST_HOOKS_ENV: &str = "TEAMAGENTS_JOB_TEST_HOOKS";

struct Runner {
    root: std::path::PathBuf,
    spec: JobSpec,
    journal: Journal,
    child: Option<Child>,
    cancel_requested_at: Option<u64>,
    fail_writes: bool,
    test_hooks: bool,
}

impl Runner {
    fn persist(&mut self) {
        if self.fail_writes {
            // Simulated disk failure: the volatile tombstone still flips in
            // memory, cancel_saved reports honestly that disk did not take it.
            self.journal.cancel_saved = false;
            return;
        }
        match atomic_json(&self.root.join("journal.json"), &self.journal) {
            Ok(()) => {}
            Err(_) => self.journal.cancel_saved = false,
        }
    }

    fn mark(&mut self, state: &str, cancel: bool) {
        self.journal.state = state.to_string();
        if cancel {
            self.journal.cancel_saved = true;
        }
        self.persist();
    }

    fn start_command(&mut self) -> Result<(), String> {
        let output = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("output.log"))
            .map_err(|e| format!("output log: {e}"))?;
        let stderr = output.try_clone().map_err(|e| format!("output log clone: {e}"))?;
        let mut command = Command::new(&self.spec.program);
        command
            .args(&self.spec.args)
            .current_dir(&self.spec.cwd)
            .env_clear()
            .envs(self.spec.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::from(output))
            .stderr(Stdio::from(stderr))
            .process_group(0);
        match command.spawn() {
            Ok(child) => {
                self.journal.pid = Some(child.id());
                self.journal.start_ticks = Some(start_ticks(child.id())?);
                self.journal.starts += 1;
                self.journal.started_ms = Some(now_ms());
                self.journal.state = "RUNNING".into();
                self.child = Some(child);
                self.persist();
                Ok(())
            }
            Err(error) => {
                self.journal.state = "FAILED".into();
                self.persist();
                Err(format!("spawn: {error}"))
            }
        }
    }

    fn request(&mut self, method: &str) -> Result<Json, String> {
        match method {
            "go" | "go-crash-after-accept" if self.journal.state == "READY" => {
                if now_ms() >= self.spec.deadline_ms {
                    return Err("command is past its deadline".into());
                }
                // Accept first, persist before any spawn (§6.2): a crash after
                // this point is OUTCOME_UNKNOWN on recovery, never "not run".
                self.journal.state = "START_ACCEPTED".into();
                self.persist();
                if method == "go-crash-after-accept" {
                    if !self.test_hooks {
                        return Err("test hooks disabled".into());
                    }
                    unsafe { libc::kill(libc::getpid(), libc::SIGKILL) };
                    std::process::abort();
                }
                self.start_command()?;
            }
            // duplicate GO never starts a second command (A10); the caller
            // re-reads the same journal
            "go" => {}
            "cancel" => {
                if self.journal.state == "READY" {
                    // persist CANCELLED_BEFORE_START first, then confirm; this
                    // terminal state permanently rejects late or replayed GOs
                    self.mark("CANCELLED_BEFORE_START", true);
                } else if self.child.is_some() {
                    self.mark("CANCEL_REQUESTED", true);
                    signal_group(&self.journal, libc::SIGTERM)?;
                    self.cancel_requested_at = Some(now_ms());
                } else if self.journal.state == "OUTCOME_UNKNOWN" && self.journal.pid.is_some() {
                    // a recovered runner cannot waitpid the old child or claim
                    // its outcome; best-effort stop with identity re-verified
                    let _ = signal_group(&self.journal, libc::SIGTERM);
                }
            }
            "status" => {}
            "fault-writes" if self.test_hooks => self.fail_writes = true,
            "repair-writes" if self.test_hooks => {
                self.fail_writes = false;
                self.persist();
            }
            "shutdown" => {
                if self.child.is_some() {
                    return Err("active command must stop before shutdown".into());
                }
            }
            other => return Err(format!("unknown runner method {other:?}")),
        }
        Ok(json!({"ok": true, "journal": self.journal, "receipt_saved": !self.fail_writes}))
    }

    /// Child reaping + deadline/cancel escalation, called on every tick.
    fn tick(&mut self) {
        if self.child.is_some() && now_ms() >= self.spec.deadline_ms && self.cancel_requested_at.is_none() {
            self.mark("CANCEL_REQUESTED", true);
            let _ = signal_group(&self.journal, libc::SIGTERM);
            self.cancel_requested_at = Some(now_ms());
        }
        if let Some(since) = self.cancel_requested_at {
            if now_ms().saturating_sub(since) > 500 && self.child.is_some() {
                let _ = signal_group(&self.journal, libc::SIGKILL);
            }
        }
        let Some(active) = self.child.as_mut() else { return };
        match active.try_wait() {
            Ok(Some(status)) => {
                use std::os::unix::process::ExitStatusExt;
                self.journal.exit_code = status.code();
                self.journal.signal = status.signal();
                let cancelled = self.journal.state == "CANCEL_REQUESTED";
                self.journal.state = if cancelled {
                    "CANCELLED".into()
                } else if status.success() {
                    "SUCCEEDED".into()
                } else {
                    "FAILED".into()
                };
                self.child = None;
                self.journal.finished_ms = Some(now_ms());
                self.persist();
            }
            Ok(None) => {}
            Err(e) => {
                self.journal.state = "OUTCOME_UNKNOWN".into();
                self.child = None;
                self.journal.finished_ms = Some(now_ms());
                self.persist();
                tracing_warn(&format!("job {} wait failed: {e}", self.journal.job_id));
            }
        }
    }
}

fn tracing_warn(message: &str) {
    eprintln!("runner: {message}");
}

/// Serve one job directory until shutdown. Recovery: an existing journal is
/// identity-checked against job.json; a runner that finds itself restarted
/// over a non-terminal state marks OUTCOME_UNKNOWN — it cannot know whether
/// the old child produced effects (§6.3).
pub async fn serve(root: &Path) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|e| format!("job dir {}: {e}", root.display()))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("runner.lock"))
        .map_err(|e| format!("runner lock: {e}"))?;
    lock.try_lock().map_err(|e| format!("job already has a live runner: {e}"))?;
    let spec: JobSpec =
        serde_json::from_slice(&std::fs::read(root.join("job.json")).map_err(|e| format!("read job.json: {e}"))?)
            .map_err(|e| format!("parse job.json: {e}"))?;
    let digest = spec.command_hash();
    let journal_path = root.join("journal.json");
    let journal = if journal_path.exists() {
        let saved: Journal =
            serde_json::from_slice(&std::fs::read(&journal_path).map_err(|e| format!("read journal: {e}"))?)
                .map_err(|e| format!("parse journal: {e}"))?;
        if saved.job_id != spec.job_id || saved.command_hash != digest {
            return Err("job identity or parameters do not match the persisted journal".into());
        }
        let mut recovered = saved;
        if matches!(recovered.state.as_str(), "START_ACCEPTED" | "RUNNING" | "CANCEL_REQUESTED") {
            recovered.state = "OUTCOME_UNKNOWN".into();
        }
        recovered
    } else {
        Journal {
            job_id: spec.job_id.clone(),
            state: "READY".into(),
            command_hash: digest,
            pid: None,
            start_ticks: None,
            boot_id: boot_id()?,
            exit_code: None,
            signal: None,
            started_ms: None,
            finished_ms: None,
            starts: 0,
            cancel_saved: false,
        }
    };
    let mut runner = Runner {
        root: root.to_path_buf(),
        spec,
        journal,
        child: None,
        cancel_requested_at: None,
        fail_writes: false,
        test_hooks: std::env::var(TEST_HOOKS_ENV).is_ok(),
    };
    runner.persist();
    let listener = UnixListener::bind_addr(&super::socket_addr(&runner.spec.token)?)
        .map_err(|e| format!("bind job socket: {e}"))?;
    let mut tick = tokio::time::interval(Duration::from_millis(10));
    loop {
        tokio::select! {
            _ = tick.tick() => runner.tick(),
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(pair) => pair,
                    Err(_) => continue,
                };
                let (read, mut write) = stream.into_split();
                let mut reader = BufReader::new(read);
                let mut line = Vec::new();
                let mut shutdown = false;
                loop {
                    line.clear();
                    let Ok(n) = reader.read_until(b'\n', &mut line).await else { break };
                    if n == 0 {
                        break;
                    }
                    let request: Json = match serde_json::from_slice(&line) {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    if request["version"].as_u64() != Some(1) {
                        let _ = write.write_all(b"{\"ok\":false,\"error\":\"protocol version mismatch\"}\n").await;
                        continue;
                    }
                    if request["token"].as_str() != Some(runner.spec.token.as_str()) {
                        let _ = write.write_all(b"{\"ok\":false,\"error\":\"job token mismatch\"}\n").await;
                        continue;
                    }
                    let method = request["method"].as_str().unwrap_or("");
                    let reply = match runner.request(method) {
                        Ok(v) => v,
                        Err(e) => json!({"ok": false, "error": e}),
                    };
                    if method == "shutdown" && reply["ok"] == json!(true) {
                        shutdown = true;
                    }
                    if write.write_all(format!("{reply}\n").as_bytes()).await.is_err() {
                        break;
                    }
                    if shutdown {
                        break;
                    }
                }
                if shutdown {
                    break;
                }
            }
        }
    }
    drop(listener);
    drop(lock);
    Ok(())
}

/// Spawn the runner as a detached child of this process (driver side). The
/// runner outlives its parent by design; reconnection goes through the
/// socket and the persisted journal, never through the child handle (A11).
pub fn spawn(root: &Path, spec: &JobSpec) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|e| format!("job dir {}: {e}", root.display()))?;
    if !root.join("job.json").exists() {
        // first spawn fixes the token; respawns keep the persisted identity
        // (a live runner from a previous incarnation rejects a second runner
        // via the lock; clients reconnect with the persisted token)
        let mut spec = spec.clone();
        if spec.token.is_empty() {
            spec.token = uuid::Uuid::new_v4().to_string();
        }
        atomic_json(&root.join("job.json"), &spec)?;
    }
    // TEAMAGENTS_RUNNER_BIN overrides the runner image for integration tests
    // (their own executable is the test harness); production uses the same
    // teamagents binary the daemon runs as (§6.2: same Rust binary).
    let executable = std::env::var_os("TEAMAGENTS_RUNNER_BIN")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_exe().expect("current exe"));
    let mut command = Command::new(executable);
    command
        .arg("jobs-runner")
        .arg(root)
        .env_remove("RUST_LOG")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    // Test hooks propagate only when the spawning test opted in.
    if std::env::var(TEST_HOOKS_ENV).is_ok() {
        command.env(TEST_HOOKS_ENV, "1");
    }
    command.spawn().map_err(|e| format!("spawn runner: {e}"))?;
    Ok(())
}
