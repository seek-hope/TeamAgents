use super::{atomic_json, ensure, init_root, now_ms, Request, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

#[derive(Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub script: String,
    pub deadline_ms: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Journal {
    pub job_id: String,
    pub state: String,
    pub command_hash: String,
    pub pid: Option<u32>,
    pub start_ticks: Option<u64>,
    pub boot_id: String,
    pub exit_code: Option<i32>,
    pub starts: u32,
    pub cancel_saved: bool,
}

fn boot_id() -> Result<String> {
    Ok(std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?.trim().into())
}

fn start_ticks(pid: u32) -> Result<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let tail = text.rsplit_once(')').ok_or("invalid process identity")?.1;
    Ok(tail.split_whitespace().nth(19).ok_or("invalid process start time")?.parse()?)
}

fn signal_group(journal: &Journal, signal: i32) -> Result<()> {
    let pid = journal.pid.ok_or("no process to verify")?;
    ensure(
        journal.boot_id == boot_id()? && journal.start_ticks == Some(start_ticks(pid)?),
        "process identity changed; refusing to signal",
    )?;
    // The group was created by this runner. Never signal a user-supplied PID.
    let result = unsafe { libc::kill(-(pid as i32), signal) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

fn persist(root: &Path, journal: &Journal, fail: bool) -> Result<()> {
    if fail {
        return Err(std::io::Error::from_raw_os_error(libc::ENOSPC).into());
    }
    atomic_json(&root.join("journal.json"), journal)
}

pub fn terminal(state: &str) -> bool {
    matches!(state, "SUCCEEDED" | "FAILED" | "CANCELLED" | "CANCELLED_BEFORE_START" | "OUTCOME_UNKNOWN")
}

pub fn spawn(root: &Path, job: &Job) -> Result<Child> {
    init_root(root)?;
    atomic_json(&root.join("job.json"), job)?;
    let child = Command::new(super::executable()?)
        .arg("runner")
        .arg(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    Ok(child)
}

pub async fn serve(root: &Path) -> Result<()> {
    init_root(root)?;
    let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(root.join("runner.lock"))?;
    lock.try_lock()?;
    let job: Job = serde_json::from_slice(&std::fs::read(root.join("job.json"))?)?;
    let digest = super::store::hash(job.script.as_bytes());
    let mut journal = if root.join("journal.json").exists() {
        let saved: Journal = serde_json::from_slice(&std::fs::read(root.join("journal.json"))?)?;
        ensure(saved.job_id == job.id && saved.command_hash == digest, "job identity or arguments do not match")?;
        saved
    } else {
        Journal {
            job_id: job.id.clone(),
            state: "READY".into(),
            command_hash: digest,
            pid: None,
            start_ticks: None,
            boot_id: boot_id()?,
            exit_code: None,
            starts: 0,
            cancel_saved: false,
        }
    };
    if matches!(journal.state.as_str(), "START_ACCEPTED" | "RUNNING" | "CANCEL_REQUESTED") {
        journal.state = "OUTCOME_UNKNOWN".into();
    }
    persist(root, &journal, false)?;
    let socket = root.join("runner.sock");
    if socket.exists() {
        std::fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    let mut child: Option<Child> = None;
    let mut fail_writes = false;
    let mut cancel_at = None;
    let mut tick = tokio::time::interval(Duration::from_millis(10));
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if child.is_some() && now_ms() >= job.deadline_ms && cancel_at.is_none() {
                    journal.state = "CANCEL_REQUESTED".into();
                    journal.cancel_saved = true;
                    if persist(root,&journal,fail_writes).is_err() { journal.cancel_saved=false; }
                    let _ = signal_group(&journal,libc::SIGTERM);
                    cancel_at=Some(now_ms());
                }
                if let Some(started) = cancel_at {
                    if now_ms().saturating_sub(started) > 100 && child.is_some() {
                        let _ = signal_group(&journal,libc::SIGKILL);
                    }
                }
                if let Some(active) = child.as_mut() {
                    if let Some(status) = active.try_wait()? {
                        journal.exit_code=status.code();
                        journal.state=if cancel_at.is_some() { "CANCELLED" } else if status.success() { "SUCCEEDED" } else { "FAILED" }.into();
                        child=None;
                        let _ = persist(root,&journal,fail_writes);
                    }
                }
            }
            accepted = listener.accept() => {
                let (stream,_) = accepted?;
                let (read,mut write)=stream.into_split();
                let mut reader=BufReader::new(read).take(65_537);
                let mut bytes=Vec::new();
                // Probe clients are local and bounded; P1 uses a bounded connection task queue.
                let request=tokio::time::timeout(Duration::from_millis(200),reader.read_until(b'\n',&mut bytes)).await;
                if !matches!(request,Ok(Ok(_))) || bytes.len()>65_536 { continue; }
                let request:Request=match serde_json::from_slice(&bytes) { Ok(r)=>r,Err(_)=>continue };
                let outcome:Result<Value>=(|| {
                    ensure(request.version==1,"protocol version mismatch")?;
                    match request.method.as_str() {
                        "go" | "go-crash-after-accept" if journal.state=="READY" => {
                            ensure(now_ms()<job.deadline_ms,"the command is past its deadline")?;
                            journal.state="START_ACCEPTED".into();
                            persist(root,&journal,fail_writes)?;
                            if request.method=="go-crash-after-accept" {
                                unsafe { libc::kill(libc::getpid(),libc::SIGKILL); }
                                std::process::abort();
                            }
                            let output=OpenOptions::new().create(true).append(true).open(root.join("output.log"))?;
                            let mut command=Command::new("/bin/sh");
                            command.arg("-c").arg(&job.script).current_dir(root)
                                .env_clear().env("PATH","/usr/bin:/bin").env("LANG","C.UTF-8")
                                .stdin(Stdio::null()).stdout(Stdio::from(output.try_clone()?)).stderr(Stdio::from(output))
                                .process_group(0);
                            match command.spawn() {
                                Ok(active)=>{
                                    journal.pid=Some(active.id());
                                    journal.start_ticks=Some(start_ticks(active.id())?);
                                    journal.starts+=1;
                                    journal.state="RUNNING".into();
                                    child=Some(active);
                                    persist(root,&journal,fail_writes)?;
                                },
                                Err(error)=>{
                                    journal.state="FAILED".into();
                                    persist(root,&journal,fail_writes)?;
                                    return Err(error.into());
                                }
                            }
                        },
                        "go" => {},
                        "cancel" => {
                            // The volatile tombstone is set even when persistence fails.
                            if journal.state=="READY" {
                                journal.state="CANCELLED_BEFORE_START".into();
                                journal.cancel_saved=true;
                                if persist(root,&journal,fail_writes).is_err() { journal.cancel_saved=false; }
                            } else if child.is_some() {
                                journal.state="CANCEL_REQUESTED".into();
                                journal.cancel_saved=true;
                                if persist(root,&journal,fail_writes).is_err() { journal.cancel_saved=false; }
                                signal_group(&journal,libc::SIGTERM)?;
                                cancel_at=Some(now_ms());
                            } else if journal.state=="OUTCOME_UNKNOWN" && journal.pid.is_some() {
                                // A recovered runner cannot waitpid the old child or claim its outcome.
                                signal_group(&journal,libc::SIGTERM)?;
                            }
                        },
                        "status" => {},
                        "fault-writes" => { fail_writes=true; },
                        "repair-writes" => { fail_writes=false; persist(root,&journal,false)?; },
                        "shutdown" => { ensure(child.is_none(),"an active command must stop first")?; },
                        _ => return Err("unknown runner method".into()),
                    }
                    Ok(json!({"ok":true,"journal":journal,"receipt_saved":!fail_writes}))
                })();
                let reply=match outcome { Ok(v)=>v,Err(e)=>json!({"ok":false,"error":e.to_string()}) };
                let _=write.write_all(format!("{reply}\n").as_bytes()).await;
                if request.method=="shutdown" && reply["ok"]==true { break; }
            }
        }
    }
    drop(listener);
    std::fs::remove_file(socket)?;
    drop(lock);
    Ok(())
}

pub fn kill_owned(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

pub fn terminate_recorded(root: &Path) {
    if let Ok(bytes) = std::fs::read(root.join("journal.json")) {
        if let Ok(journal) = serde_json::from_slice::<Journal>(&bytes) {
            let _ = signal_group(&journal, libc::SIGKILL);
        }
    }
}

pub fn read_journal(root: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&std::fs::read(root.join("journal.json"))?)?)
}

pub fn file_lock(root: &Path) -> Result<File> {
    let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(root.join("daemon.lock"))?;
    lock.try_lock()?;
    Ok(lock)
}
