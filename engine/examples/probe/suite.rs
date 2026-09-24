use super::{atomic_json, ensure, executable, init_root, io_probe, now_ms, rpc, rpc_id, runner, store::Store, Result};
use serde_json::{json, Value};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Processes(Vec<(Child, PathBuf)>);
impl Processes {
    fn add(&mut self, child: Child, root: &Path) -> usize {
        self.0.push((child, root.into()));
        self.0.len() - 1
    }
    fn kill(&mut self, index: usize) {
        runner::kill_owned(&mut self.0[index].0);
    }
}
impl Drop for Processes {
    fn drop(&mut self) {
        for (child, root) in &mut self.0 {
            runner::terminate_recorded(root);
            runner::terminate_recorded(&root.join("job"));
            runner::kill_owned(child);
        }
    }
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn ready(socket: &Path) -> Result<Value> {
    let until = now_ms() + 4000;
    loop {
        if let Ok(value) = rpc(socket, "status") {
            return Ok(value);
        }
        ensure(now_ms() < until, &format!("socket not ready: {}", socket.display()))?;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn settled(root: &Path) -> Result<Value> {
    let until = now_ms() + 4000;
    loop {
        let value = rpc(&root.join("runner.sock"), "status")?;
        if runner::terminal(value["journal"]["state"].as_str().unwrap_or("")) {
            return Ok(value["journal"].clone());
        }
        ensure(now_ms() < until, "the command did not reach a terminal state in time")?;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn job(script: &str) -> runner::Job {
    runner::Job { id: uuid::Uuid::new_v4().to_string(), script: script.into(), deadline_ms: now_ms() + 8000 }
}

fn daemon_process(root: &Path) -> Result<Child> {
    Ok(Command::new(executable()?)
        .arg("daemon")
        .arg(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(std::fs::File::create(root.join("daemon.log"))?))
        .spawn()?)
}

fn transactions(root: &Path) -> Result<Value> {
    let mut results = Vec::new();
    for point in ["before", "after"] {
        let case = root.join(point);
        let store = Store::open(&case)?;
        store.ingest("input", "durable user input")?;
        drop(store);
        let status = Command::new(executable()?)
            .args(["transaction-child", case.to_str().ok_or("the path is not UTF-8")?, point])
            .status()?;
        ensure(status.signal() == Some(libc::SIGKILL), "the child was not killed at the chosen failure point")?;
        let mut recovered = Store::open(&case)?;
        let before: i64 = recovered.db.query_row("SELECT COUNT(*) FROM context", [], |r| r.get(0))?;
        ensure(before == if point == "before" { 0 } else { 1 }, "wrong state at the commit failure point")?;
        recovered.apply("input", None)?;
        recovered.apply("input", None)?;
        let counts:(i64,i64,i64)=recovered.db.query_row(
            "SELECT (SELECT COUNT(*) FROM context),(SELECT COUNT(*) FROM events),(SELECT revision FROM execution WHERE id='instance')",
            [],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        ensure(counts == (1, 1, 1), "after restart, input was applied twice or events disagree")?;
        ensure(
            recovered.ingest("input", "changed payload").is_err(),
            "the same input id accepted a different payload",
        )?;
        results.push(json!({"crash":point,"before_recovery_rows":before,"after_recovery":counts}));
    }
    Ok(json!(results))
}

fn artifacts(root: &Path) -> Result<Value> {
    let status = Command::new(executable()?).arg("artifact-child").arg(root).status()?;
    ensure(status.signal() == Some(libc::SIGKILL), "the artifact publication failure point did not trigger")?;
    let mut store = Store::open(root)?;
    ensure(store.collect()? == 0, "GC deleted a response that was not imported yet")?;
    store.attach("response", "request")?;
    ensure(store.collect()? == 0, "GC deleted a live reference")?;
    store.reserve_artifact("orphan", "failed-request", b"unused")?;
    store.publish("orphan", b"unused")?;
    store.abandon("orphan")?;
    ensure(store.claim_gc()? == vec!["orphan"], "wrong GC claim")?;
    ensure(store.attach("orphan", "late-reference").is_err(), "a new reference was allowed after the GC claim")?;
    drop(store);
    let mut store = Store::open(root)?;
    ensure(store.collect()? == 1, "collection did not continue across a restart after the claim")?;
    ensure(
        std::fs::read(root.join("artifacts/response"))? == b"complete-response",
        "a committed reference is unreadable",
    )?;
    ensure(!root.join("artifacts/orphan").exists(), "the orphan artifact was not collected")?;
    store.reserve_artifact("unfinished-job", "runner-job", b"receipt")?;
    store.publish("unfinished-job", b"receipt")?;
    ensure(store.collect()? == 0, "an unimported job artifact was not protected")?;
    store.attach("unfinished-job", "operation")?;
    Ok(
        json!({"publication_sigkill":true,"staging_survives_gc":true,"claimed_object_rejects_reference":true,"gc_restart":true,"job_result_pin":true}),
    )
}

fn sqlite_full() -> Result<Value> {
    let db = rusqlite::Connection::open_in_memory()?;
    db.execute_batch("PRAGMA page_size=512; CREATE TABLE t(payload BLOB);")?;
    let pages: i64 = db.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    db.pragma_update(None, "max_page_count", pages)?;
    let error = db.execute("INSERT INTO t VALUES(zeroblob(8192))", []).expect_err("SQLITE_FULL not injected");
    ensure(error.sqlite_error_code() == Some(rusqlite::ErrorCode::DiskFull), "the injected error is not SQLITE_FULL")?;
    Ok(json!({"error":"SQLITE_FULL","host_disk_filled":false}))
}

async fn runners(root: &Path, processes: &mut Processes) -> Result<Value> {
    let before = root.join("cancel-before");
    let spec = job("printf x >> effects");
    let index = processes.add(runner::spawn(&before, &spec)?, &before);
    let socket = before.join("runner.sock");
    ready(&socket).await?;
    let mut store = Store::open(&root.join("operations"))?;
    store.prepare_job(&spec.id)?;
    store.dispatch(&spec.id)?;
    store.cancel(&spec.id)?;
    rpc(&socket, "cancel")?;
    processes.kill(index);
    processes.add(runner::spawn(&before, &spec)?, &before);
    ready(&socket).await?;
    ensure(store.recovery_method(&spec.id)? == "cancel", "recovery re-sent a stale GO")?;
    rpc(&socket, "go")?;
    let value = rpc(&socket, "go")?;
    ensure(
        value["journal"]["starts"] == 0 && value["journal"]["state"] == "CANCELLED_BEFORE_START",
        "the command started despite the cancellation",
    )?;
    ensure(!before.join("effects").exists(), "an external effect appeared before the cancellation")?;

    let duplicate = root.join("duplicate");
    let spec = job("printf x >> effects; sleep 0.15");
    processes.add(runner::spawn(&duplicate, &spec)?, &duplicate);
    let socket = duplicate.join("runner.sock");
    ready(&socket).await?;
    store.prepare_job(&spec.id)?;
    store.dispatch(&spec.id)?;
    rpc(&socket, "go")?;
    rpc(&socket, "go")?;
    let receipt = settled(&duplicate).await?;
    ensure(receipt["state"] == "SUCCEEDED" && receipt["starts"] == 1, "a duplicate GO ran the command more than once")?;
    ensure(std::fs::read(duplicate.join("effects"))? == b"x", "wrong effect count")?;
    store.import_receipt(&spec.id, &receipt)?;
    store.import_receipt(&spec.id, &receipt)?;
    let receipts: i64 = store.db.query_row("SELECT COUNT(*) FROM events WHERE kind='receipt'", [], |r| r.get(0))?;
    ensure(receipts == 1, "the receipt was consumed twice")?;

    let failure = root.join("storage-failure");
    let spec = job("sleep 20 & echo $! > descendant; wait");
    let index = processes.add(runner::spawn(&failure, &spec)?, &failure);
    let socket = failure.join("runner.sock");
    ready(&socket).await?;
    store.prepare_job(&spec.id)?;
    store.dispatch(&spec.id)?;
    rpc(&socket, "go")?;
    store.db.pragma_update(None, "query_only", true)?;
    ensure(store.cancel(&spec.id).is_err(), "the failing cancel write was not injected")?;
    rpc(&socket, "fault-writes")?;
    let started = Instant::now();
    let cancelled = rpc(&socket, "cancel")?;
    ensure(cancelled["journal"]["cancel_saved"] == false, "wrongly claimed the cancellation was saved")?;
    let receipt = settled(&failure).await?;
    ensure(receipt["state"] == "CANCELLED", "the failed write blocked the real stop")?;
    let cancel_ms = started.elapsed().as_secs_f64() * 1000.0;
    ensure(runner::read_journal(&failure)?["state"] == "RUNNING", "the fault injection left no unsaved state")?;
    processes.kill(index);
    processes.add(runner::spawn(&failure, &spec)?, &failure);
    ready(&socket).await?;
    let recovered = rpc(&socket, "go")?;
    ensure(
        recovered["journal"]["state"] == "OUTCOME_UNKNOWN" && recovered["journal"]["starts"] == 1,
        "an unsaved cancellation was wrongly replayed after restart",
    )?;
    store.db.pragma_update(None, "query_only", false)?;

    let unknown = root.join("unknown");
    let spec = job("printf x >> effects; sleep 20");
    let index = processes.add(runner::spawn(&unknown, &spec)?, &unknown);
    let socket = unknown.join("runner.sock");
    ready(&socket).await?;
    rpc(&socket, "go")?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    processes.kill(index);
    processes.add(runner::spawn(&unknown, &spec)?, &unknown);
    ready(&socket).await?;
    let status = rpc(&socket, "go")?;
    ensure(status["journal"]["state"] == "OUTCOME_UNKNOWN", "the runner crash was guessed as success, or replayed")?;
    rpc(&socket, "cancel")?;
    ensure(std::fs::read(unknown.join("effects"))? == b"x", "an unknown outcome was executed again")?;

    let accepted = root.join("accepted-no-pid");
    let spec = job("printf x >> effects");
    let index = processes.add(runner::spawn(&accepted, &spec)?, &accepted);
    let socket = accepted.join("runner.sock");
    ready(&socket).await?;
    ensure(rpc(&socket, "go-crash-after-accept").is_err(), "the start failure point did not trigger")?;
    let status = processes.0[index].0.wait()?;
    ensure(status.signal() == Some(libc::SIGKILL), "the runner did not crash after accepting GO")?;
    processes.add(runner::spawn(&accepted, &spec)?, &accepted);
    ready(&socket).await?;
    let status = rpc(&socket, "go")?;
    ensure(
        status["journal"]["state"] == "OUTCOME_UNKNOWN" && !accepted.join("effects").exists(),
        "an unknown start without a pid was replayed",
    )?;

    let timed = root.join("deadline");
    let mut spec = job("exec sleep 20");
    spec.deadline_ms = now_ms() + 300;
    processes.add(runner::spawn(&timed, &spec)?, &timed);
    ready(&timed.join("runner.sock")).await?;
    rpc(&timed.join("runner.sock"), "go")?;
    ensure(settled(&timed).await?["state"] == "CANCELLED", "the runner did not enforce the deadline on its own")?;

    Ok(json!({"cancel_before_go_restart":true,"duplicate_go_one_effect":true,"receipt_import_once":true,
        "storage_failure_stop_ms":cancel_ms,"cancel_persistence_reported_false":true,"unknown_after_runner_crash":true,
        "accepted_without_pid_not_replayed":true,"runner_deadline_independent":true}))
}

async fn background(root: &Path, processes: &mut Processes) -> Result<Value> {
    init_root(root)?;
    let job_root = root.join("job");
    init_root(&job_root)?;
    let spec = job("printf x >> effects; sleep 0.8");
    atomic_json(&job_root.join("job.json"), &spec)?;
    let first = processes.add(daemon_process(root)?, root);
    let socket = root.join("daemon.sock");
    ready(&socket).await?;
    let started = rpc_id(&socket, "start", "stable-start-command")?;
    tokio::time::sleep(Duration::from_millis(130)).await;
    let before = rpc(&socket, "status")?;
    ensure(before["ticks"].as_u64().unwrap_or(0) > 0, "a waiting instance blocked an active one")?;
    let lock_result = runner::file_lock(root);
    ensure(lock_result.is_err(), "a second coordinator was allowed")?;
    processes.kill(first);
    let second = processes.add(daemon_process(root)?, root);
    ready(&socket).await?;
    let replay = rpc_id(&socket, "start", "stable-start-command")?;
    ensure(started == replay, "a client retry did not reuse the same receipt")?;
    let receipt = settled(&job_root).await?;
    ensure(
        receipt["state"] == "SUCCEEDED" && receipt["starts"] == 1,
        "a daemon crash replayed or killed the independent runner",
    )?;
    ensure(std::fs::read(job_root.join("effects"))? == b"x", "daemon recovery duplicated a side effect")?;
    let paused = rpc(&socket, "pause")?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    ensure(rpc(&socket, "status")?["ticks"] == paused["ticks"], "work advanced while paused")?;
    rpc(&socket, "resume")?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    ensure(
        rpc(&socket, "status")?["ticks"].as_u64() > paused["ticks"].as_u64(),
        "resume did not continue the same work",
    )?;
    rpc(&socket, "cancel")?;
    rpc(&socket, "shutdown")?;
    let _ = processes.0[second].0.wait()?;
    processes.add(daemon_process(root)?, root);
    ready(&socket).await?;
    let store = Store::open(root)?;
    let state: String = store.db.query_row("SELECT state FROM operations WHERE id=?1", [&spec.id], |r| r.get(0))?;
    ensure(state == "TERMINAL", "the restart did not import the runner terminal receipt")?;
    ensure(rpc(&socket, "status")?["status"] == "CANCELLED", "the restart revived a cancelled job")?;
    rpc(&socket, "shutdown")?;
    rpc(&job_root.join("runner.sock"), "shutdown")?;
    Ok(json!({"daemon_sigkill":true,"runner_survives":true,"single_effect":true,"command_receipt_stable":true,
        "pause_resume_cancel":true,"lock_not_inherited":true,"waiting_instance_nonblocking":true}))
}

fn storage_benchmark(root: &Path) -> Result<Value> {
    let mut store = Store::open(root)?;
    let body = "unit ".repeat(1_000_000);
    store.reserve_artifact("history", "benchmark", body.as_bytes())?;
    store.publish("history", body.as_bytes())?;
    store.attach("history", "context")?;
    store.db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    let initial = std::fs::metadata(root.join("session.sqlite"))?.len();
    let mut latency = Vec::new();
    let cpu = io_probe::cpu_us();
    for index in 0..120 {
        let start = Instant::now();
        let id = format!("step-{index}");
        store.ingest(&id, "artifact:history; small appended observation")?;
        store.apply(&id, None)?;
        latency.push(start.elapsed().as_micros() as u64);
    }
    store.db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    let after = std::fs::metadata(root.join("session.sqlite"))?.len();
    ensure(after - initial < 1_000_000, "each step copied a large history through storage")?;
    let build = Instant::now();
    let request = serde_json::to_vec(&json!({"messages":[{"role":"user","content":body}]}))?;
    let request_us = build.elapsed().as_micros();
    drop(store);
    let reopen = Instant::now();
    let mut recovered = Store::open(root)?;
    recovered.attach("history", "context")?;
    let recovery_us = reopen.elapsed().as_micros();
    Ok(json!({"synthetic_units":1_000_000,"synthetic_bytes":5_000_000,"real_tokenizer":false,
        "steps":120,"append_apply_us":io_probe::percentiles(latency),"database_growth_bytes":after-initial,
        "request_bytes":request.len(),"request_build_us":request_us,"reopen_verify_us":recovery_us,
        "cpu_us":io_probe::cpu_us().saturating_sub(cpu),"rss_kib":io_probe::process_metric("VmRSS:"),
        "peak_rss_kib":io_probe::process_metric("VmHWM:"),"synchronous":"FULL","journal_mode":"WAL"}))
}

pub async fn run(output: PathBuf) -> Result<()> {
    ensure(!output.exists(), "the evidence directory exists already; use a new one")?;
    std::fs::create_dir_all(&output)?;
    let output = std::fs::canonicalize(output)?;
    let scratch = Scratch(std::env::temp_dir().join(format!("tap0-{}", &uuid::Uuid::new_v4().to_string()[..8])));
    std::fs::create_dir(&scratch.0)?;
    let mut processes = Processes(Vec::new());
    let mut report =
        json!({"phase":"probe","status":"running","model_calls":0,"sqlite_version":rusqlite::version(),"checks":{}});
    atomic_json(&output.join("report.json"), &report)?;
    let result: Result<()> = async {
        ensure(rusqlite::version_number() >= 3_051_003, "this SQLite lacks the required WAL fix")?;
        report["checks"]["sqlite_full"] = sqlite_full()?;
        report["checks"]["atomic_input"] = transactions(&scratch.0.join("transactions"))?;
        report["checks"]["artifacts_gc"] = artifacts(&scratch.0.join("artifacts"))?;
        report["checks"]["runner"] = runners(&scratch.0, &mut processes).await?;
        report["checks"]["background"] = background(&scratch.0.join("daemon"), &mut processes).await?;
        report["storage"] = storage_benchmark(&output.join("storage"))?;
        report["io"] = io_probe::run().await?;
        Ok(())
    }
    .await;
    report["status"] = json!(if result.is_ok() { "passed" } else { "failed" });
    if let Err(error) = &result {
        report["error"] = json!(error.to_string());
    }
    atomic_json(&output.join("report.json"), &report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    drop(processes);
    drop(scratch);
    result
}
