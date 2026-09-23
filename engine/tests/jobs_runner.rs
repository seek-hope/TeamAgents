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
    client::spawn(&dir, &spec("op-deadline", "sleep 300", future(300))).await.expect("spawn");
    client::go(&dir).await.expect("go");
    assert_eq!(wait_terminal(&dir, 10_000).await, "CANCELLED");
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
