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
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};

/// Test-only fault injection is compiled in but disabled unless the parent
/// explicitly opts in per runner process (never from the job file or model).
const TEST_HOOKS_ENV: &str = "TEAMAGENTS_JOB_TEST_HOOKS";

/// How often the runner looks at its child. The loop costs CPU in *every* running job — measured at a 10 ms
/// interval it was 1.80 % of a core per command (D-116) — and the only thing the cadence buys is how late a
/// completed command, a past deadline or the TERM→KILL escalation is noticed:
///
/// * a command's exit is reported to the model up to one tick later,
/// * a deadline is enforced up to one tick late,
/// * the escalation is `CANCEL_ESCALATION_MS` plus at most one tick.
///
/// 50 ms keeps all three well under a tenth of a second (and the escalation within one tick of its budget),
/// while cutting the loop's cost by five.
pub(crate) const TICK: Duration = Duration::from_millis(50);

/// How often an **idle** runner looks at anything: with no child and no cancel in flight there is nothing a
/// timer can notice — the tick's three jobs (a command's exit, a past deadline, the TERM→KILL escalation) all
/// need a child — so the loop only has to answer a client that connects and to notice the two states in which
/// waiting can only leak the process: a job directory that is gone, and a job that is settled. It used to keep
/// the 50 ms tick forever, which is what a *leaked* runner cost: measured 2026-09-26, every crashed or
/// abandoned job left a process burning 0.40 % of a core for as long as it lived, and one machine had
/// accumulated **1,397** of them (oldest 57 h, ~5–11 cores in total; 988 of them had a `SUCCEEDED` journal,
/// 250 `FAILED`, 70 `CANCELLED` — a settled job whose runner nobody ever shut down) because nothing ever told
/// them to stop (D-153). At this cadence the same process costs one wakeup every 30 s, and it is also the
/// grace a settled job gets before its runner goes away.
pub(crate) const IDLE_TICK: Duration = Duration::from_secs(30);

/// Test-only override for [`IDLE_TICK`]: a test cannot wait 30 s for a rule about waiting.
const IDLE_TICK_ENV: &str = "TEAMAGENTS_JOB_IDLE_TICK_MS";

fn idle_tick() -> Duration {
    std::env::var(IDLE_TICK_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(IDLE_TICK)
}

/// How long a cancelled command gets between TERM and KILL (§6.4's "stop request, then escalate").
pub(crate) const CANCEL_ESCALATION_MS: u64 = 500;

struct Runner {
    root: std::path::PathBuf,
    spec: JobSpec,
    journal: Journal,
    child: Option<Child>,
    cancel_requested_at: Option<u64>,
    /// Read once at startup (see [`IDLE_TICK_ENV`]): a test sets it before spawning, and the runner keeps the
    /// value its parent gave it even after the parent restores its own environment.
    idle_tick: Duration,
    fail_writes: bool,
    test_hooks: bool,
    /// Whether the last journal write landed: what every reply reports as `receipt_saved`. A runner that cannot
    /// write must not report what disk did not take (V2DiskFull's `NoFakedSuccess`, A31), and a *real*
    /// `atomic_json` failure (`EACCES`, a full disk) is the same fact as the simulated fault — before D-305 the
    /// reply read `!self.fail_writes`, so a real failure was reported as a saved receipt.
    journal_saved: bool,
}

impl Runner {
    fn persist(&mut self) {
        if self.fail_writes {
            // Simulated disk failure: the volatile tombstone still flips in
            // memory, cancel_saved reports honestly that disk did not take it.
            self.journal.cancel_saved = false;
            self.journal_saved = false;
            return;
        }
        match atomic_json(&self.root.join("journal.json"), &self.journal) {
            Ok(()) => self.journal_saved = true,
            Err(_) => {
                self.journal.cancel_saved = false;
                self.journal_saved = false;
            }
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
                self.journal.finished_ms = Some(now_ms());
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
                if !self.journal_saved {
                    // An in-memory acceptance cannot anchor recovery after a
                    // crash. No process has started, so this attempt can fail
                    // honestly even when the failure journal also cannot save.
                    self.journal.finished_ms = Some(now_ms());
                    self.mark("FAILED", false);
                    return Err("start acceptance could not be persisted; command was not started".into());
                }
                if method == "go-crash-after-accept" {
                    if !self.test_hooks {
                        return Err("test hooks disabled".into());
                    }
                    // SAFETY: this is the test hook's deliberate SIGKILL of its own process; the hook only
                    // runs when explicitly enabled, and the following abort is unreachable.
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
                    self.journal.finished_ms = Some(now_ms());
                    self.mark("CANCELLED_BEFORE_START", true);
                } else if self.child.is_some() && self.cancel_requested_at.is_none() {
                    self.mark("CANCEL_REQUESTED", true);
                    self.cancel_requested_at = Some(now_ms());
                    signal_group(&self.journal, libc::SIGTERM)?;
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
        Ok(json!({"ok": true, "journal": self.journal, "receipt_saved": self.journal_saved}))
    }

    /// Child reaping + deadline/cancel escalation, called on every tick.
    fn tick(&mut self) {
        if self.cancel_requested_at.is_none() {
            // Completion already observable at this poll precedes a new timeout.
            // An unreaped successful leader must not expose its surviving service
            // to cancellation just because the next tick arrived after the deadline.
            self.reap_child();
            if self.child.is_none() {
                return;
            }
        }
        if self.child.is_some() && now_ms() >= self.spec.deadline_ms && self.cancel_requested_at.is_none() {
            self.mark("CANCEL_REQUESTED", true);
            let _ = signal_group(&self.journal, libc::SIGTERM);
            self.cancel_requested_at = Some(now_ms());
        }
        if let Some(since) = self.cancel_requested_at {
            // Keep even an exited leader unreaped until escalation. Its PID and
            // start ticks remain an identity anchor for the entire group; reaping
            // it early would strand descendants that ignored TERM. Normal
            // completion still permits intentionally persistent services.
            if now_ms().saturating_sub(since) < CANCEL_ESCALATION_MS {
                return;
            }
            let _ = signal_group(&self.journal, libc::SIGKILL);
            self.reap_child();
        }
    }

    fn reap_child(&mut self) {
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
                // the escalation target is gone, so nothing needs a timer any more: without this the loop
                // stays "busy" forever after a cancel and never reaches the idle rules (D-153)
                self.cancel_requested_at = None;
                self.journal.finished_ms = Some(now_ms());
                self.persist();
            }
            Ok(None) => {}
            Err(e) => {
                self.journal.state = "OUTCOME_UNKNOWN".into();
                self.child = None;
                self.cancel_requested_at = None;
                self.journal.finished_ms = Some(now_ms());
                self.persist();
                tracing_warn(&format!("job {} wait failed: {e}", self.journal.job_id));
            }
        }
    }

    /// Has this job been settled long enough that a runner adds nothing to it?
    ///
    /// A settled job is fully readable without a runner: the journal, the output and the receipt are files
    /// (`client::read_output`) and a client that finds no runner judges from the persisted journal (§6.3, A11)
    /// — which is exactly what recovery does after a crash. So the only thing the grace buys is the client that
    /// is *watching* the job as it settles: it polls every few hundred ms (D-116), so one idle cadence is
    /// generous by an order of magnitude, and it keeps a machine from accumulating settled runners forever
    /// (D-153: 1,308 of the 1,397 measured had a terminal journal).
    fn settled_long_enough(&self) -> bool {
        self.journal
            .finished_ms
            .is_some_and(|finished| now_ms().saturating_sub(finished) >= self.idle_tick.as_millis() as u64)
    }
}

struct RunnerRequest {
    method: String,
    reply: oneshot::Sender<Json>,
}

/// Socket waits never own the runner state. Each connection has bounded input
/// and waits; requests still commit serially in the supervising loop.
async fn serve_client(stream: UnixStream, token: String, requests: mpsc::Sender<RunnerRequest>) -> bool {
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    loop {
        let mut line = Vec::new();
        let read = tokio::time::timeout(super::RPC_TIMEOUT, async {
            (&mut reader).take(super::MAX_RPC_BYTES + 1).read_until(b'\n', &mut line).await
        })
        .await;
        if !matches!(read, Ok(Ok(n)) if n > 0)
            || line.len() as u64 > super::MAX_RPC_BYTES
            || line.last() != Some(&b'\n')
        {
            return false;
        }
        let Ok(request) = serde_json::from_slice::<Json>(&line) else { return false };
        let method = request["method"].as_str().unwrap_or("");
        let reply = if request["version"].as_u64() != Some(1) {
            json!({"ok":false,"error":"protocol version mismatch"})
        } else if request["token"].as_str() != Some(token.as_str()) {
            json!({"ok":false,"error":"job token mismatch"})
        } else {
            let (reply, response) = oneshot::channel();
            let exchange = tokio::time::timeout(super::RPC_TIMEOUT, async {
                requests.send(RunnerRequest { method: method.to_string(), reply }).await.ok()?;
                response.await.ok()
            })
            .await;
            let Ok(Some(reply)) = exchange else { return false };
            reply
        };
        let shutdown = method == "shutdown" && reply["ok"] == json!(true);
        let written = tokio::time::timeout(super::RPC_TIMEOUT, write.write_all(format!("{reply}\n").as_bytes())).await;
        if shutdown || !matches!(written, Ok(Ok(()))) {
            return shutdown;
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
        if recovered.terminal() && recovered.finished_ms.is_none() {
            recovered.finished_ms = Some(now_ms());
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
        idle_tick: idle_tick(),
        fail_writes: false,
        test_hooks: std::env::var(TEST_HOOKS_ENV).is_ok(),
        journal_saved: true,
    };
    runner.persist();
    let listener = UnixListener::bind_addr(&super::socket_addr(&runner.spec.token)?)
        .map_err(|e| format!("bind job socket: {e}"))?;
    let (requests, mut pending) = mpsc::channel::<RunnerRequest>(32);
    let mut clients = tokio::task::JoinSet::new();
    let tick = tokio::time::sleep(runner.idle_tick);
    tokio::pin!(tick);
    let mut idle_timer = true;
    let mut shutting_down = false;
    loop {
        let idle = runner.child.is_none() && runner.cancel_requested_at.is_none();
        // Client traffic cannot move the deadline. Only a change between idle
        // and active supervision changes the cadence.
        if idle != idle_timer {
            tick.as_mut().reset(tokio::time::Instant::now() + if idle { runner.idle_tick } else { TICK });
            idle_timer = idle;
        }
        if shutting_down && clients.is_empty() {
            break;
        }
        tokio::select! {
            biased;
            _ = &mut tick => {
                if !runner.root.is_dir() || (idle && runner.journal.terminal() && runner.settled_long_enough()) {
                    break;
                }
                runner.tick();
                tick.as_mut().reset(tokio::time::Instant::now() + if idle { runner.idle_tick } else { TICK });
            }
            Some(request) = pending.recv() => {
                let reply = if shutting_down {
                    json!({"ok":false,"error":"runner is shutting down"})
                } else {
                    match runner.request(&request.method) {
                        Ok(reply) => reply,
                        Err(error) => json!({"ok":false,"error":error}),
                    }
                };
                shutting_down |= request.method == "shutdown" && reply["ok"] == json!(true);
                let _ = request.reply.send(reply);
            }
            finished = clients.join_next(), if !clients.is_empty() => {
                if matches!(finished, Some(Ok(true))) {
                    break;
                }
            }
            accepted = listener.accept(), if !shutting_down => {
                if let Ok((stream, _)) = accepted {
                    if clients.len() < 32 {
                        clients.spawn(serve_client(stream, runner.spec.token.clone(), requests.clone()));
                    }
                }
            }
        }
    }
    clients.abort_all();
    while clients.join_next().await.is_some() {}
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_observed_exit_wins_over_a_late_deadline_tick_and_preserves_its_service() {
        let root = std::env::temp_dir().join(format!("teamagents-exit-deadline-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let spec = JobSpec {
            job_id: "exit-before-poll".into(),
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "sleep 30 &".into()],
            cwd: root.to_string_lossy().into_owned(),
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            deadline_ms: u64::MAX,
            token: "unused".into(),
        };
        let journal = Journal {
            job_id: spec.job_id.clone(),
            state: "START_ACCEPTED".into(),
            command_hash: spec.command_hash(),
            pid: None,
            start_ticks: None,
            boot_id: boot_id().unwrap(),
            exit_code: None,
            signal: None,
            started_ms: None,
            finished_ms: None,
            starts: 0,
            cancel_saved: false,
        };
        let mut runner = Runner {
            root: root.clone(),
            spec,
            journal,
            child: None,
            cancel_requested_at: None,
            idle_tick: IDLE_TICK,
            fail_writes: false,
            test_hooks: false,
            journal_saved: true,
        };
        runner.start_command().unwrap();
        let pid = runner.journal.pid.unwrap();
        let mut exited = false;
        for _ in 0..400 {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
            if stat.rsplit_once(") ").is_some_and(|(_, rest)| rest.starts_with("Z ")) {
                exited = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        // The leader exited before this deliberately late supervision tick.
        // It remains unreaped, so the old ordering would still cancel its group.
        runner.spec.deadline_ms = now_ms().saturating_sub(1);
        runner.tick();
        let state = runner.journal.state.clone();
        let cancelled = runner.cancel_requested_at.is_some();
        let live = super::super::group_members(&super::super::proc_table(), pid).len();
        // SAFETY: this process group was created by this test and its 30-second
        // descendant is still owned by the fixture; no system-wide process match.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
        if let Some(mut child) = runner.child.take() {
            let _ = child.wait();
        }
        std::fs::remove_dir_all(root).unwrap();
        assert!(exited, "the fixture must observe an unreaped completed leader");
        assert_eq!(state, "SUCCEEDED");
        assert!(!cancelled && live > 0, "a completed command's service must survive its late poll");
    }
}
