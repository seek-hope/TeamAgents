use super::*;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

async fn isolated_runner(dir: &Path, job: &JobSpec) -> std::process::Child {
    let mut job = job.clone();
    job.token = uuid::Uuid::new_v4().to_string();
    teamagents_engine::jobs::atomic_json(&dir.join("job.json"), &job).unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_teamagents"))
        .arg("jobs-runner")
        .arg(dir)
        .env("TEAMAGENTS_JOB_TEST_HOOKS", "1")
        .env("TEAMAGENTS_JOB_IDLE_TICK_MS", "200")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..200 {
        if client::status(dir).await.is_ok() {
            return child;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("isolated runner did not start");
}

#[tokio::test]
async fn a_failed_start_acceptance_write_never_launches_the_command() {
    let dir = root("accept-write-failure");
    let mut job = spec("op-no-durable-accept", "echo effect > effect", future(30_000));
    job.cwd = dir.to_string_lossy().into_owned();
    let mut child = isolated_runner(&dir, &job).await;
    client::inject(&dir, "fault-writes").await.unwrap();
    let start = client::go(&dir).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let effect = dir.join("effect").exists();
    let _ = client::inject(&dir, "repair-writes").await;
    let _ = client::shutdown(&dir).await;
    let _ = child.wait();
    assert!(
        start.is_err() && !effect,
        "acceptance must be durable before any process starts: {start:?}, effect={effect}"
    );
}

#[tokio::test]
async fn every_terminal_path_has_a_retirement_timestamp() {
    for mode in ["prestart", "spawn-failed", "recovered"] {
        let dir = root(mode);
        let mut job = spec(mode, "true", future(30_000));
        if mode == "spawn-failed" {
            job.program = dir.join("missing-program").to_string_lossy().into_owned();
        }
        if mode == "recovered" {
            teamagents_engine::jobs::atomic_json(
                &dir.join("journal.json"),
                &teamagents_engine::jobs::Journal {
                    job_id: job.job_id.clone(),
                    state: "START_ACCEPTED".into(),
                    command_hash: job.command_hash(),
                    pid: None,
                    start_ticks: None,
                    boot_id: teamagents_engine::jobs::boot_id().unwrap(),
                    exit_code: None,
                    signal: None,
                    started_ms: None,
                    finished_ms: None,
                    starts: 0,
                    cancel_saved: false,
                },
            )
            .unwrap();
        }
        let mut child = isolated_runner(&dir, &job).await;
        if mode == "prestart" {
            client::cancel(&dir).await.unwrap();
        }
        if mode == "spawn-failed" {
            assert!(client::go(&dir).await.is_err());
        }
        let journal = client::persisted_journal(&dir).unwrap();
        let mut retired = false;
        for _ in 0..100 {
            if child.try_wait().unwrap().is_some() {
                retired = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let _ = client::shutdown(&dir).await;
        let _ = child.wait();
        assert!(
            journal.terminal() && journal.finished_ms.is_some() && retired,
            "{mode}: {journal:?}, retired={retired}"
        );
    }
}

#[test]
fn output_preview_reads_only_the_requested_prefix() {
    const CHILD_ROOT: &str = "TA_OUTPUT_PREVIEW_TEST_ROOT";
    if let Some(dir) = std::env::var_os(CHILD_ROOT) {
        assert_eq!(client::read_output(Path::new(&dir), 6), "prefix");
        return;
    }
    use std::io::Write;
    use std::os::unix::process::CommandExt;
    let dir = root("bounded-output");
    let mut file = std::fs::File::create(dir.join("output.log")).unwrap();
    file.write_all(b"prefix").unwrap();
    file.set_len(1024 * 1024 * 1024).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", "jobs_robustness::output_preview_reads_only_the_requested_prefix"])
        .env(CHILD_ROOT, dir.as_os_str());
    // SAFETY: the child changes only its own address-space limit before exec;
    // setrlimit uses a stack value and no allocation or inherited lock.
    unsafe {
        child.pre_exec(|| {
            let limit = libc::rlimit { rlim_cur: 256 * 1024 * 1024, rlim_max: 256 * 1024 * 1024 };
            if libc::setrlimit(libc::RLIMIT_AS, &limit) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    assert!(child.output().unwrap().status.success(), "a small preview must not allocate the full 1 GiB log");
}

#[tokio::test]
async fn a_stalled_client_cannot_starve_status_or_deadline_supervision() {
    runner_bin();
    let dir = root("stalled-client");
    client::spawn(&dir, &spec("op-stall", "sleep 30", future(2_000))).await.unwrap();
    client::go(&dir).await.unwrap();
    let saved: JobSpec = serde_json::from_slice(&std::fs::read(dir.join("job.json")).unwrap()).unwrap();
    let mut stream = tokio::net::UnixStream::connect_addr(&teamagents_engine::jobs::socket_addr(&saved.token).unwrap())
        .await
        .unwrap();
    stream
        .write_all(format!("{}\n", serde_json::json!({"version":1,"token":saved.token,"method":"status"})).as_bytes())
        .await
        .unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    assert!(line.contains("RUNNING"));
    let responsive = tokio::time::timeout(Duration::from_millis(500), client::status(&dir)).await.is_ok();
    // Wait for the actual terminal journal rather than assuming the 2 s
    // deadline has already elapsed on a loaded CI runner.
    let settled = wait_terminal(&dir, 5_000).await;
    drop(reader);
    // Cleanup before asserting so the failing old implementation leaves no job.
    let _ = client::cancel(&dir).await;
    let _ = wait_terminal(&dir, 5_000).await;
    let _ = client::shutdown(&dir).await;
    assert!(responsive, "another connection must remain responsive");
    assert_eq!(settled, "CANCELLED", "a stalled client must not postpone the job deadline");
}

#[tokio::test]
async fn cancellation_escalates_before_reaping_a_term_exiting_leader() {
    runner_bin();
    let scratch = root("cancel-descendant");
    let dir = scratch.join("jobs/descendant");
    let mut job = spec(
        "descendant",
        "trap 'exit 0' TERM; (trap '' TERM; echo ready > ready; exec sleep 30) & wait",
        future(30_000),
    );
    job.cwd = scratch.to_string_lossy().into_owned();
    client::spawn(&dir, &job).await.unwrap();
    client::go(&dir).await.unwrap();
    for _ in 0..200 {
        if scratch.join("ready").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(scratch.join("ready").exists());
    client::cancel(&dir).await.unwrap();
    assert_eq!(wait_terminal(&dir, 5_000).await, "CANCELLED");
    for _ in 0..100 {
        if group_len(&dir) == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let remaining = group_len(&dir);
    // The old runner settles after reaping only its leader. Explicit cleanup
    // still has the original birth-time evidence for the surviving service.
    let _ = runners(&scratch, RunnersCommand::Stop { job_id: None, service: true }).await;
    let _ = client::shutdown(&dir).await;
    assert_eq!(remaining, 0, "TERM-resistant descendants must receive escalation");
}

#[tokio::test]
async fn repeated_cancel_does_not_replenish_the_escalation_grace() {
    runner_bin();
    let dir = root("cancel-repeat");
    client::spawn(&dir, &spec("op-repeat", "trap '' TERM; echo ready; exec sleep 30", future(30_000))).await.unwrap();
    client::go(&dir).await.unwrap();
    for _ in 0..200 {
        if client::read_output(&dir, 32).contains("ready") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    client::cancel(&dir).await.unwrap();
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(40)).await;
        let _ = client::cancel(&dir).await;
    }
    let settled = client::persisted_journal(&dir).unwrap().terminal();
    let _ = wait_terminal(&dir, 5_000).await;
    let _ = client::shutdown(&dir).await;
    assert!(settled, "duplicate cancellation must not move the first escalation deadline");
}
