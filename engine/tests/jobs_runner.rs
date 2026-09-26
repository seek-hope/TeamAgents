//! R2-P2 job runner contract tests (§6.2/§6.3): READY/GO/CANCEL handshake,
//! duplicate GO dedup, cancel-before-start persistence, crash recovery and
//! OUTCOME_UNKNOWN honesty. Real processes on the local machine, no model.

use std::path::{Path, PathBuf};
use teamagents_engine::jobs::{client, JobSpec};

fn root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("teamagents-jobs-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn spec(job_id: &str, script: &str, deadline_ms: u64) -> JobSpec {
    JobSpec {
        job_id: job_id.into(),
        program: "/bin/bash".into(),
        args: vec!["--noprofile".into(), "--norc".into(), "-c".into(), script.into()],
        cwd: "/tmp".into(),
        env: vec![
            ("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into()),
            ("HOME".into(), "/tmp".into()),
            ("LANG".into(), "C.UTF-8".into()),
        ],
        deadline_ms,
        token: String::new(),
    }
}

fn future(ms_from_now: u64) -> u64 {
    teamagents_engine::jobs::now_ms() + ms_from_now
}

/// Point the runner spawn at the real teamagents binary (integration tests
/// run as their own executable).
fn runner_bin() {
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
}

async fn wait_terminal(dir: &Path, timeout_ms: u64) -> String {
    for _ in 0..(timeout_ms / 50) {
        if let Ok(journal) = client::status(dir).await {
            if journal.terminal() {
                return journal.state;
            }
        } else if let Some(journal) = client::persisted_journal(dir) {
            if journal.terminal() {
                return journal.state;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("job did not reach a terminal state within {timeout_ms}ms");
}

#[tokio::test]
async fn happy_path_ready_go_success_with_output() {
    runner_bin();
    let dir = root("happy");
    client::spawn(&dir, &spec("op-happy", "echo hello-v2", future(30_000))).await.expect("spawn");
    let ready = client::status(&dir).await.expect("status");
    assert_eq!(ready.state, "READY");
    let journal = client::go(&dir).await.expect("go");
    assert_eq!(journal.state, "RUNNING");
    assert_eq!(wait_terminal(&dir, 10_000).await, "SUCCEEDED");
    let journal = client::status(&dir).await.expect("final status");
    assert_eq!(journal.exit_code, Some(0));
    assert!(client::read_output(&dir, 1_000_000).contains("hello-v2"));
    client::shutdown(&dir).await.expect("shutdown");
}

#[tokio::test]
async fn duplicate_go_starts_exactly_one_command() {
    runner_bin();
    let dir = root("dup");
    let marker = dir.join("starts.txt");
    let script = format!("echo start >> {}; sleep 0.2", marker.display());
    client::spawn(&dir, &spec("op-dup", &script, future(30_000))).await.expect("spawn");
    // sequential duplicate: second GO after RUNNING is a deduped no-op
    client::go(&dir).await.expect("go 1");
    let second = client::go(&dir).await.expect("go 2");
    assert_eq!(second.starts, 1, "duplicate GO must not start a second command");
    assert_eq!(wait_terminal(&dir, 10_000).await, "SUCCEEDED");
    // replayed GO after the terminal state is still deduped
    let replayed = client::go(&dir).await.expect("go 3");
    assert_eq!(replayed.state, "SUCCEEDED");
    assert_eq!(replayed.starts, 1);
    let starts = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(starts.matches("start").count(), 1);
    client::shutdown(&dir).await.expect("shutdown");
}

#[tokio::test]
async fn cancel_before_start_persists_and_rejects_late_go() {
    runner_bin();
    let dir = root("cbs");
    let marker = dir.join("ran.txt");
    let script = format!("touch {}", marker.display());
    client::spawn(&dir, &spec("op-cbs", &script, future(30_000))).await.expect("spawn");
    let cancelled = client::cancel(&dir).await.expect("cancel");
    assert_eq!(cancelled.state, "CANCELLED_BEFORE_START");
    assert!(cancelled.cancel_saved, "cancel intent must reach disk before confirmation");
    // the terminal tombstone permanently rejects late or replayed GO
    let late = client::go(&dir).await.expect("late go");
    assert_eq!(late.state, "CANCELLED_BEFORE_START");
    assert_eq!(late.starts, 0);
    assert!(!marker.exists(), "a cancelled-before-start command never runs");
    client::shutdown(&dir).await.expect("shutdown");
}

#[tokio::test]
async fn cancel_running_stops_the_process_group() {
    runner_bin();
    let dir = root("cancel");
    client::spawn(&dir, &spec("op-cancel", "sleep 300", future(300_000))).await.expect("spawn");
    client::go(&dir).await.expect("go");
    let pid = client::status(&dir).await.expect("status").pid.expect("pid");
    client::cancel(&dir).await.expect("cancel");
    assert_eq!(wait_terminal(&dir, 10_000).await, "CANCELLED");
    // the real process is gone, not just forgotten
    assert!(!Path::new(&format!("/proc/{pid}")).exists(), "process {pid} survived the cancel");
    client::shutdown(&dir).await.expect("shutdown");
}

#[tokio::test]
async fn deadline_cancels_a_stuck_command() {
    runner_bin();
    let dir = root("deadline");
    // `future` takes milliseconds. The deadline must be short enough to cancel the
    // 300-second sleep promptly and long enough that `go` cannot arrive after it: with
    // 300 ms a loaded suite (every test binary in parallel) made the spawn+go
    // round-trips exceed it once, and the runner then answered - correctly - "command
    // is past its deadline" instead of running, which failed the test. The behaviour
    // under test is the *runner's* deadline, not the test's own latency.
    client::spawn(&dir, &spec("op-deadline", "sleep 300", future(2_000))).await.expect("spawn");
    client::go(&dir).await.expect("go");
    assert_eq!(wait_terminal(&dir, 10_000).await, "CANCELLED");
    client::shutdown(&dir).await.expect("shutdown");
}

/// The deadline refuses a late GO (`go` after the deadline answers "command is past
/// its deadline" and nothing runs). This was only ever observed by accident — a
/// flaky sibling test whose 300 ms deadline passed before its own `go` arrived — so
/// it gets a deliberate, deterministic test with a deadline that is already past.
#[tokio::test]
async fn go_past_the_deadline_is_refused_and_runs_nothing() {
    runner_bin();
    let dir = root("late-go");
    let past = teamagents_engine::jobs::now_ms().saturating_sub(1_000);
    // the trace lands inside this test's own directory (an absolute path), so its
    // absence really means the command did not run
    let trace = dir.join("late-go-ran");
    client::spawn(&dir, &spec("op-late", &format!("printf ran > {}", trace.display()), past)).await.expect("spawn");
    let refused = client::go(&dir).await.expect_err("a late go must be refused");
    assert!(refused.contains("past its deadline"), "{refused}");
    // the job never ran: the journal stays READY and the command left no trace
    let journal = client::status(&dir).await.expect("status");
    assert_eq!(journal.state, "READY", "the command must not have started");
    assert!(!trace.exists(), "the command must not have run: {}", trace.display());
    client::shutdown(&dir).await.expect("shutdown");
}

#[tokio::test]
async fn runner_crash_after_accept_recovers_as_outcome_unknown() {
    runner_bin();
    std::env::set_var("TEAMAGENTS_JOB_TEST_HOOKS", "1");
    let dir = root("crash");
    client::spawn(&dir, &spec("op-crash", "sleep 1", future(30_000))).await.expect("spawn");
    // the runner accepts GO, persists START_ACCEPTED, then dies by SIGKILL
    let _ = client::inject(&dir, "go-crash-after-accept").await;
    // wait for the runner process to die (socket stops responding)
    for _ in 0..100 {
        if client::status(&dir).await.is_err() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let persisted = client::persisted_journal(&dir).expect("persisted journal");
    assert_eq!(persisted.state, "START_ACCEPTED");
    // recovery: a fresh runner over the same dir must not guess "not run"
    client::spawn(&dir, &spec("op-crash", "sleep 1", future(30_000))).await.expect("respawn");
    let recovered = client::status(&dir).await.expect("recovered status");
    assert_eq!(recovered.state, "OUTCOME_UNKNOWN");
    client::shutdown(&dir).await.expect("shutdown");
}

#[tokio::test]
async fn daemon_crash_reconnects_the_same_job_without_restart() {
    runner_bin();
    let dir = root("reconnect");
    let script = "echo reconnect-ok; sleep 0.3";
    client::spawn(&dir, &spec("op-reconnect", script, future(30_000))).await.expect("spawn");
    client::go(&dir).await.expect("go");
    // simulate a daemon crash: every client handle is dropped; the runner and
    // its journal survive independently (§6.2)
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let journal = client::status(&dir).await.expect("reconnect status");
    assert!(journal.starts == 1, "reconnect must not start a second command");
    let again = client::go(&dir).await.expect("reconnect go");
    assert_eq!(again.starts, 1);
    assert_eq!(wait_terminal(&dir, 10_000).await, "SUCCEEDED");
    assert!(client::read_output(&dir, 1_000_000).contains("reconnect-ok"));
    client::shutdown(&dir).await.expect("shutdown");
}

#[tokio::test]
async fn second_runner_over_a_live_job_is_refused() {
    runner_bin();
    let dir = root("lock");
    client::spawn(&dir, &spec("op-lock", "sleep 0.1", future(30_000))).await.expect("spawn");
    // a direct second serve over the same dir fails on the runner lock (A10:
    // at most one sanctioned executor)
    let result = teamagents_engine::jobs::runner::serve(&dir).await;
    assert!(result.is_err(), "second runner must be refused");
    client::shutdown(&dir).await.expect("shutdown");
}

/// A12/D-41: a successful command that starts a background service finishes
/// promptly — output goes to a file, never a held pipe — and the service,
/// in the command's own process group and never signalled on success,
/// outlives both the job and the runner (across calls and CLI exits).
#[tokio::test]
async fn a_successful_commands_service_outlives_the_job() {
    runner_bin();
    let dir = root("service");
    let pidfile = dir.join("svc.pid");
    let script = format!("sleep 300 & echo $! > {}; echo service-started", pidfile.display());
    client::spawn(&dir, &spec("op-svc", &script, future(30_000))).await.expect("spawn");
    client::go(&dir).await.expect("go");
    assert_eq!(wait_terminal(&dir, 10_000).await, "SUCCEEDED");
    let pid: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
    let alive = || std::path::Path::new(&format!("/proc/{pid}")).exists();
    assert!(alive(), "the service survives job completion");
    client::shutdown(&dir).await.expect("shutdown");
    assert!(alive(), "the service survives the runner shutdown");
    unsafe { libc::kill(pid, libc::SIGKILL) };
}

/// The runner process serving `dir`, by its argument list: `jobs-runner <dir>` (a client-started runner is
/// detached, so the test has no `Child` handle for it), skipping a corpse that keeps the name in this
/// container.
fn runner_pid(dir: &Path) -> Option<u32> {
    let needle = dir.to_string_lossy().into_owned();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else { continue };
        let Ok(raw) = std::fs::read(entry.path().join("cmdline")) else { continue };
        let argv: Vec<String> = raw
            .split(|byte| *byte == 0)
            .filter(|chunk| !chunk.is_empty())
            .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
            .collect();
        if argv.get(1).map(String::as_str) != Some("jobs-runner") || !argv.contains(&needle) {
            continue;
        }
        let stat = std::fs::read_to_string(entry.path().join("stat")).unwrap_or_default();
        if !stat.split_whitespace().nth(2).is_some_and(|field| field.starts_with('Z')) {
            return Some(pid);
        }
    }
    None
}

/// D-153: a runner nobody will ever talk to again must not wait forever, and there are two shapes of that.
///
/// * the **job directory is gone**: every client resolves the runner's socket from the token in `<job
///   dir>/job.json`, and nothing the runner records can be written any more. Measured on one machine: 19 of
///   1,397 live runners were in this state.
/// * the **job is settled**: the journal, the output and the receipt are files, and a client that finds no
///   runner judges from the persisted journal (§6.3/A11) — that is what recovery does after a crash. So a
///   settled job's runner only has to outlive the client watching it settle. Measured: 1,308 of those 1,397
///   had a terminal journal (`SUCCEEDED` 988, `FAILED` 250, `CANCELLED` 70) and were never shut down.
///
/// Each shape also has its control: a runner whose job is *reachable and unsettled* keeps serving, and it does
/// not exit just because some cadence passed. Before this, every one of those processes kept the 50 ms
/// running-job tick forever — 0.40 % of a core each, ~5–11 cores on that machine (D-153).
#[tokio::test]
async fn a_runner_waits_while_its_job_is_reachable_and_unsettled_and_exits_otherwise() {
    // a test cannot wait 30 s for a rule about waiting: the runner reads this at startup (test-only knob,
    // restored before the assertions so nothing else in this binary inherits it)
    std::env::set_var("TEAMAGENTS_JOB_IDLE_TICK_MS", "200");
    runner_bin();

    // --- control: a reachable, *unsettled* job keeps its runner, well past several idle cadences ----------
    let dir = root("idle-waits");
    client::spawn(&dir, &spec("op-idle", "sleep 30", future(60_000))).await.expect("spawn");
    std::env::remove_var("TEAMAGENTS_JOB_IDLE_TICK_MS");
    client::go(&dir).await.expect("go");
    let pid = runner_pid(&dir).expect("the runner is a live process while its job is reachable");
    for _ in 0..10 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(client::status(&dir).await.is_ok(), "a reachable, unsettled runner must keep serving");
    }
    assert_eq!(runner_pid(&dir), Some(pid), "the reachable runner is still there after several cadences");

    // --- the job directory disappears: nothing can reach this runner any more ------------------------------
    let journal = client::status(&dir).await.expect("status before the removal");
    std::fs::remove_dir_all(&dir).expect("remove the job directory");
    let gone = wait_gone(&dir, 5_000).await;
    assert!(gone, "runner {pid} kept waiting after its job directory disappeared");
    if let Some(child) = journal.pid {
        // the command is in its own process group and survives its runner (A12): stop it as well
        let _ = std::process::Command::new("kill").arg(child.to_string()).status();
    }

    // --- a cancelled job is the same shape: its runner goes away too (before this, the cancel path left
    //     `cancel_requested_at` set, so the loop never became idle and the process kept the 50 ms tick forever;
    //     that is the shape of the 68 CANCELLED runners found on this machine, D-153) ------------------------
    let cancelled_dir = root("idle-cancelled");
    std::env::set_var("TEAMAGENTS_JOB_IDLE_TICK_MS", "200");
    client::spawn(&cancelled_dir, &spec("op-cancelled", "sleep 30", future(60_000))).await.expect("spawn");
    std::env::remove_var("TEAMAGENTS_JOB_IDLE_TICK_MS");
    client::go(&cancelled_dir).await.expect("go");
    let cancelled_pid = runner_pid(&cancelled_dir).expect("the runner is there while the command runs");
    client::cancel(&cancelled_dir).await.expect("cancel");
    assert_eq!(wait_terminal(&cancelled_dir, 10_000).await, "CANCELLED");
    assert!(wait_gone(&cancelled_dir, 5_000).await, "runner {cancelled_pid} kept waiting after its job was cancelled");

    // --- the job settles: the runner goes away on its own, one grace after the settlement ------------------
    let settled_dir = root("idle-settled");
    std::env::set_var("TEAMAGENTS_JOB_IDLE_TICK_MS", "200");
    client::spawn(&settled_dir, &spec("op-settled", "true", future(30_000))).await.expect("spawn");
    std::env::remove_var("TEAMAGENTS_JOB_IDLE_TICK_MS");
    client::go(&settled_dir).await.expect("go");
    assert_eq!(wait_terminal(&settled_dir, 10_000).await, "SUCCEEDED");
    let settled_pid = runner_pid(&settled_dir).expect("the runner is there while the command finishes");
    assert!(wait_gone(&settled_dir, 5_000).await, "runner {settled_pid} kept waiting after its job settled");
    // and the job is still fully readable without a runner: the files are the record (A11)
    assert_eq!(client::persisted_journal(&settled_dir).expect("journal").state, "SUCCEEDED");
}

/// Poll until the runner serving `dir` is gone, up to `timeout_ms`.
async fn wait_gone(dir: &Path, timeout_ms: u64) -> bool {
    for _ in 0..(timeout_ms / 100) {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if runner_pid(dir).is_none() {
            return true;
        }
    }
    false
}
