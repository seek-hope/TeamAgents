//! R2-P2 job runner contract tests (§6.2/§6.3): READY/GO/CANCEL handshake,
//! duplicate GO dedup, cancel-before-start persistence, crash recovery and
//! OUTCOME_UNKNOWN honesty. Real processes on the local machine, no model.

use std::path::Path;
use teamagents_engine::jobs::{client, JobSpec};
use teamagents_engine::v2::runners::{RunnersCommand, RunnersOptions};

mod common;

/// The scratch root of one test, removed when the test ends (D-226).
fn root(tag: &str) -> common::TempRoot {
    common::TempRoot::new(&format!("jobs-{tag}"))
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

async fn runners(root: &Path, command: RunnersCommand) -> serde_json::Value {
    teamagents_engine::v2::runners::execute_async(&RunnersOptions {
        state_root: root.to_path_buf(),
        command,
        json_out: true,
        confirmed: true,
    })
    .await
    .expect("runners")
}

/// The live members of a job's process group, from the journal's own recorded child.
fn group_len(dir: &Path) -> usize {
    let journal = client::persisted_journal(dir).expect("journal");
    let Some(group) = journal.pid else { return 0 };
    teamagents_engine::jobs::group_members(&teamagents_engine::jobs::proc_table(), group).len()
}

/// D-251: the other leftover — the service a settled command left behind (a child started with `&` whose shell
/// is gone). It cannot be *asked* anything (no runner supervises it any more), so it is a signal, and the
/// identity rule is the members' birth time rather than A15's child check.
#[tokio::test]
async fn the_runners_verb_stops_the_service_a_settled_command_left_behind() {
    runner_bin();
    let scratch = root("service");
    let dir = scratch.join("jobs/op-service");
    // the shell exits at once and leaves a child behind: the classic `dev-server &`
    client::spawn(&dir, &spec("op-service", "sleep 20 & echo started", future(30_000))).await.expect("spawn");
    client::go(&dir).await.expect("go");
    assert_eq!(wait_terminal(&dir, 10_000).await, "SUCCEEDED");

    // the census sees the leftover group, and the signal needs the caller's `--yes`
    let listed = runners(&scratch, RunnersCommand::List).await;
    assert_eq!(listed["runners"][0]["service"], serde_json::json!(1), "{listed}");
    let unconfirmed = teamagents_engine::v2::runners::execute_async(&RunnersOptions {
        state_root: scratch.to_path_buf(),
        command: RunnersCommand::Stop { job_id: None, service: true },
        json_out: true,
        confirmed: false,
    })
    .await;
    match unconfirmed {
        Err((2, message)) => assert!(message.contains("--yes"), "{message}"),
        other => panic!("a service stop without --yes must be a usage error: {other:?}"),
    }
    assert_eq!(group_len(&dir), 1, "the service is alive before the stop");

    // the stop lands on the group the command created
    let stopped = runners(&scratch, RunnersCommand::Stop { job_id: Some("op-service".into()), service: true }).await;
    let outcome = stopped["runners"][0]["outcome"].as_str().unwrap_or("");
    assert!(outcome.starts_with("service stopped:"), "{stopped}");
    for _ in 0..100 {
        if group_len(&dir) == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(group_len(&dir), 0, "the signalled group is gone");
    // and a second stop reports the truth instead of signalling nothing twice
    let again = runners(&scratch, RunnersCommand::Stop { job_id: None, service: true }).await;
    assert_eq!(again["runners"][0]["outcome"], serde_json::json!("no service left"), "{again}");

    // the same stop with the runner already *gone* (which is the usual case — a settled runner retires itself):
    // the service lever must not depend on a runner being there to answer
    let second = root("service-no-runner");
    let dir2 = second.join("jobs/op-service");
    client::spawn(&dir2, &spec("op-service", "sleep 20 & echo started", future(30_000))).await.expect("spawn");
    client::go(&dir2).await.expect("go");
    assert_eq!(wait_terminal(&dir2, 10_000).await, "SUCCEEDED");
    client::shutdown(&dir2).await.expect("retire the runner first");
    assert!(client::status(&dir2).await.is_err(), "the runner is gone, the service is not");
    assert_eq!(group_len(&dir2), 1, "the service outlives its runner");
    let stopped = runners(&second, RunnersCommand::Stop { job_id: None, service: true }).await;
    assert_eq!(stopped["runners"][0]["runner"], serde_json::json!("gone"), "{stopped}");
    assert!(
        stopped["runners"][0]["outcome"].as_str().unwrap_or("").starts_with("service stopped:"),
        "a gone runner must not stop the service lever: {stopped}"
    );
}

/// D-251's other half, and the one that matters most: a command that may still be *running* is not a service.
#[tokio::test]
async fn a_service_stop_refuses_a_job_that_is_not_settled() {
    runner_bin();
    let scratch2 = root("service-live");
    let dir = scratch2.join("jobs/op-live");
    client::spawn(&dir, &spec("op-live", "sleep 3", future(30_000))).await.expect("spawn");
    client::go(&dir).await.expect("go");
    let refused = runners(&scratch2, RunnersCommand::Stop { job_id: None, service: true }).await;
    let outcome = refused["runners"][0]["outcome"].as_str().unwrap_or("");
    assert!(outcome.starts_with("refused:"), "{refused}");
    assert!(outcome.contains("not settled"), "the reason names what it is: {refused}");
    assert_eq!(group_len(&dir), 1, "the running command's group is untouched");
    assert_eq!(wait_terminal(&dir, 15_000).await, "SUCCEEDED");
    client::shutdown(&dir).await.expect("shutdown");
}

/// D-250: the `runners` verb over a real state root — what it lists, and the one thing it must never do: take a
/// running command away. The lever talks to the runner itself (the token in the job directory addresses its
/// socket), so it needs no daemon — and this state root has none, which the report says with `session_id: null`.
#[tokio::test]
async fn the_runners_verb_lists_a_state_roots_jobs_and_never_breaks_a_running_command() {
    runner_bin();
    let root = root("verb");
    let dir = root.join("jobs/op-verb");
    client::spawn(&dir, &spec("op-verb", "sleep 2; echo done", future(30_000))).await.expect("spawn");
    client::go(&dir).await.expect("go");

    // what the state root carries: one job, its runner live, the command's child named
    let listed = runners(&root, RunnersCommand::List).await;
    assert_eq!(listed["session_id"], serde_json::json!(null), "{listed}");
    let rows = listed["runners"].as_array().expect("rows");
    assert_eq!(rows.len(), 1, "{listed}");
    assert_eq!(rows[0]["job_id"], serde_json::json!("op-verb"), "{listed}");
    assert_eq!(rows[0]["state"], serde_json::json!("RUNNING"), "{listed}");
    assert_eq!(rows[0]["terminal"], serde_json::json!(false), "{listed}");
    assert_eq!(rows[0]["runner"], serde_json::json!("live"), "{listed}");
    assert!(rows[0]["child_pid"].as_u64().is_some(), "the command's pid is reported: {listed}");

    // stopping it is refused by the runner's own gate, and the command keeps running
    let refused = runners(&root, RunnersCommand::Stop { job_id: None, service: false }).await;
    let outcome = refused["runners"][0]["outcome"].as_str().unwrap_or("");
    assert!(outcome.starts_with("refused:"), "{refused}");
    assert!(outcome.contains("active command"), "the reason is the runner's own: {refused}");
    assert_eq!(refused["runners"][0]["state"], serde_json::json!("RUNNING"), "{refused}");

    // the command finishes on its own (the runner outlives the daemon and journals it), and now the same verb
    // retires the runner instead of refusing
    assert_eq!(wait_terminal(&dir, 15_000).await, "SUCCEEDED");
    let retired = runners(&root, RunnersCommand::Stop { job_id: None, service: false }).await;
    assert_eq!(retired["runners"][0]["outcome"], serde_json::json!("retired"), "{retired}");
    assert!(client::status(&dir).await.is_err(), "the retired runner answers nothing");

    // a second stop reports the truth instead of inventing work, and an unknown id is a usage error
    let again = runners(&root, RunnersCommand::Stop { job_id: Some("op-verb".into()), service: false }).await;
    assert_eq!(again["runners"][0]["runner"], serde_json::json!("gone"), "{again}");
    let missing = teamagents_engine::v2::runners::execute_async(&RunnersOptions {
        state_root: root.to_path_buf(),
        command: RunnersCommand::Stop { job_id: Some("no-such-job".into()), service: false },
        json_out: true,
        confirmed: true,
    })
    .await;
    match missing {
        Err((2, message)) => assert!(message.contains("no-such-job"), "{message}"),
        other => panic!("an unknown job id must be exit 2: {other:?}"),
    }
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

/// A13's completion race, and the design line behind it (`docs/DESIGN.md`: "cancellation never overwrites a real
/// effect that happened"): a CANCEL that arrives after the command settled changes nothing — the journal keeps
/// its outcome, its exit code and its single start. The cancel arm acts only on `READY` (before any start) or on
/// a live child, so a settled job's cancel is a no-op; the pre-start and deadline races have their own tests
/// above. Nothing drove this half before: the cited `jobs_runner::cancel_*` cannot even reach it, because the
/// command is already gone.
#[tokio::test]
async fn a_cancel_after_the_command_settled_never_overwrites_the_outcome() {
    runner_bin();
    let dir = root("cancel-settled");
    client::spawn(&dir, &spec("op-settled", "true", future(30_000))).await.expect("spawn");
    client::go(&dir).await.expect("go");
    assert_eq!(wait_terminal(&dir, 10_000).await, "SUCCEEDED");
    let before = client::status(&dir).await.expect("status before the cancel");
    let cancelled = client::cancel(&dir).await.expect("a cancel after the command settled");
    assert_eq!(cancelled.state, "SUCCEEDED", "a settled command's cancel must not overwrite its outcome");
    let after = client::status(&dir).await.expect("status after the cancel");
    assert_eq!(after.state, "SUCCEEDED");
    assert_eq!(after.exit_code, before.exit_code, "the settled exit code stands");
    assert_eq!(after.starts, before.starts, "and no second start was recorded");
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

/// A15's *verification* half: the runner re-reads `/proc` before it signals, so a journal whose pid has been
/// recycled (the number exists, the process is not the job's) is refused rather than signalled. The row claims
/// the runner "persists **and verifies** pid + boot_id + start_ticks"; the persistence and the live identity are
/// driven by the other runner tests and the probe, and nothing drove the refusal — the half that keeps a cancel
/// from killing an unrelated process. Signal 0 delivers nothing, so the guard is driven without signalling.
#[test]
fn a_journal_whose_process_identity_moved_is_refused_not_signalled() {
    use std::os::unix::process::CommandExt;
    // a harmless process in its own group, so `kill(-pid, 0)` can answer "this group exists"
    let mut child = std::process::Command::new("sleep")
        .arg("300")
        .process_group(0)
        .spawn()
        .expect("spawn a harmless child in its own group");
    let pid = child.id();
    let real_boot = teamagents_engine::jobs::boot_id().expect("boot id");
    let real_ticks = teamagents_engine::jobs::start_ticks(pid).expect("start ticks");
    let journal = |boot: String, ticks: Option<u64>| teamagents_engine::jobs::Journal {
        job_id: "op-identity".into(),
        state: "RUNNING".into(),
        command_hash: "h".into(),
        pid: Some(pid),
        start_ticks: ticks,
        boot_id: boot,
        exit_code: None,
        signal: None,
        started_ms: None,
        finished_ms: None,
        starts: 1,
        cancel_saved: false,
    };
    // the matching identity is accepted (and signal 0 delivers nothing)
    teamagents_engine::jobs::signal_group(&journal(real_boot.clone(), Some(real_ticks)), 0)
        .expect("the recorded identity is the process the journal names");
    // a recycled pid: the same number, a different process
    let moved = teamagents_engine::jobs::signal_group(&journal(real_boot.clone(), Some(real_ticks + 1)), 0)
        .expect_err("a recycled pid must be refused");
    assert!(moved.contains("identity changed"), "{moved}");
    // the same pid after a reboot is a different process as well
    let rebooted = teamagents_engine::jobs::signal_group(
        &journal("00000000-0000-0000-0000-000000000000".into(), Some(real_ticks)),
        0,
    )
    .expect_err("a journal from another boot must be refused");
    assert!(rebooted.contains("identity changed"), "{rebooted}");
    let _ = child.kill();
    let _ = child.wait();
}

/// A31's runner side. The row's parenthetical says the write fault is covered by "the job runner's
/// `TEAMAGENTS_JOB_TEST_HOOKS`", and until this test the only `fault-writes` user was
/// `engine/examples/probe/`'s *separate copy* of the backend, which no target runs. Both faults are one rule
/// (V2DiskFull's `NoFakedSuccess`, A31's own citation): a runner that cannot write must not report what disk did
/// not take. The simulated fault was honest by construction; a *real* failure was not — the reply read
/// `!self.fail_writes`, so an `EACCES` (or a full disk) was reported as a saved receipt (D-305).
#[tokio::test]
async fn a_write_fault_is_never_reported_as_a_saved_receipt() {
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    runner_bin();
    // The var is read by the runner at startup; another test in this binary arms the same hook, and nothing
    // removes it, so this test leaves it set rather than toggling it.
    std::env::set_var("TEAMAGENTS_JOB_TEST_HOOKS", "1");

    // (a) the documented fault hook
    let hook = root("write-fault-hook");
    client::spawn(&hook, &spec("op-hook", "sleep 300", future(300_000))).await.expect("spawn");
    let armed = client::inject(&hook, "fault-writes").await.expect("arm the write fault");
    assert_eq!(armed["ok"], json!(true), "{armed}");
    // `receipt_saved` is what the last journal *write* did, so it is the reply to the method that writes — the
    // cancel's tombstone — that has to be honest
    let cancelled = client::inject(&hook, "cancel").await.expect("cancel");
    assert_eq!(cancelled["journal"]["cancel_saved"], json!(false), "disk did not take the intent: {cancelled}");
    assert_eq!(cancelled["receipt_saved"], json!(false), "the armed fault must not be reported as a save: {cancelled}");

    // (b) a real write failure: the job directory made unwritable, so every `atomic_json` from here fails with
    // `EACCES` exactly as it would on a full disk
    let real = root("write-fault-real");
    client::spawn(&real, &spec("op-real", "sleep 300", future(300_000))).await.expect("spawn");
    client::go(&real).await.expect("go");
    let mode = std::fs::metadata(&real).expect("job dir").permissions().mode();
    let mut locked = std::fs::metadata(&real).expect("job dir").permissions();
    locked.set_mode(0o500);
    std::fs::set_permissions(&real, locked).expect("make the job directory unwritable");
    let reply = client::inject(&real, "cancel").await.expect("cancel under a real write failure");
    assert_eq!(reply["receipt_saved"], json!(false), "a real failure must not be reported as a save: {reply}");
    assert_eq!(reply["journal"]["cancel_saved"], json!(false), "{reply}");
    let on_disk = client::persisted_journal(&real).expect("the journal disk really holds");
    assert_eq!(on_disk.state, "RUNNING", "disk kept the last state it took: {on_disk:?}");
    let mut open = std::fs::metadata(&real).expect("job dir").permissions();
    open.set_mode(mode);
    std::fs::set_permissions(&real, open).expect("restore the job directory");

    // stop what is left: the command outlives its runner (A12), so kill it and let the runner reap it (a sparse
    // poll — a dense one starves the runner's own tick, measured) before this test's scratch root goes away
    for (job, reply) in [(&hook, &cancelled), (&real, &reply)] {
        if let Some(pid) = reply["journal"]["pid"].as_u64() {
            let _ = std::process::Command::new("kill").args(["-9", &pid.to_string()]).status();
        }
        for _ in 0..20 {
            if client::status(job).await.map(|journal| journal.terminal()).unwrap_or(false) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        let _ = client::shutdown(job).await;
    }
}
