//! R2 job runner (plan §6.2): one controlled runner process per active shell
//! command. The runner is independent of the daemon/driver lifetime, holds
//! the unique job lock, and journals the start handshake (READY → GO →
//! RUNNING → terminal) so a crash never has to guess whether a side effect
//! happened. Duplicate GO never starts a second command; CANCEL before start
//! persists CANCELLED_BEFORE_START first and permanently rejects late GOs.
//!
//! Recovery contract (§6.3): the driver reconnects the same job via the
//! socket; a dead runner is judged from its persisted journal — missing PID
//! after START_ACCEPTED is OUTCOME_UNKNOWN, never "did not run".

pub mod client;
pub mod runner;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The fixed dispatch input for one job (written by the driver as job.json).
/// Built from the same `ShellCommandSpec` the synchronous shell path uses,
/// so runner and direct execution are byte-identical (A15).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobSpec {
    /// job_id == operation_id of the owning operation.
    pub job_id: String,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: Vec<(String, String)>,
    /// Wall-clock deadline (ms since Unix epoch); the runner cancels the
    /// process group past it.
    pub deadline_ms: u64,
    /// Job token (§6.2): authenticates control requests; the abstract socket
    /// name derives from it, so a guessed name never reaches the runner.
    pub token: String,
}

impl JobSpec {
    /// Identity hash over the dispatch-relevant fields; a runner refuses a
    /// job file whose content does not match its journal (§6.2).
    pub fn command_hash(&self) -> String {
        let mut hasher = Sha256::new();
        // the token authenticates control; it is not part of the command
        // identity (respawn keeps the persisted identity intact)
        let canonical = serde_json::json!({
            "job_id": self.job_id,
            "program": self.program,
            "args": self.args,
            "cwd": self.cwd,
            "env": self.env,
        });
        hasher.update(canonical.to_string().as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

/// The runner's persisted truth (journal.json). Terminal states are written
/// before any reply, so a recovered daemon can import the real outcome.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Journal {
    pub job_id: String,
    /// READY | START_ACCEPTED | RUNNING | CANCEL_REQUESTED |
    /// SUCCEEDED | FAILED | CANCELLED | CANCELLED_BEFORE_START | OUTCOME_UNKNOWN
    pub state: String,
    pub command_hash: String,
    pub pid: Option<u32>,
    /// /proc start ticks of the child; pid alone never proves identity.
    pub start_ticks: Option<u64>,
    pub boot_id: String,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    /// RUNNING entry and terminal-state times (ms); receipt durations come
    /// from these, never from the driver's wall clock.
    pub started_ms: Option<u64>,
    pub finished_ms: Option<u64>,
    pub starts: u32,
    /// The cancel intent reached disk (§6.4: persist before signalling).
    pub cancel_saved: bool,
}

impl Journal {
    pub fn terminal(&self) -> bool {
        terminal(&self.state)
    }
}

pub fn terminal(state: &str) -> bool {
    matches!(state, "SUCCEEDED" | "FAILED" | "CANCELLED" | "CANCELLED_BEFORE_START" | "OUTCOME_UNKNOWN")
}

/// Abstract-namespace socket address for a job token: immune to job-dir path
/// length (a 108-byte filesystem socket fails deep under state roots), and it
/// disappears with the listener — no stale socket files after crashes.
#[cfg(target_os = "linux")]
pub fn socket_addr(token: &str) -> Result<tokio::net::unix::SocketAddr, String> {
    let mut hasher = Sha256::new();
    hasher.update(b"teamagents-job:");
    hasher.update(token.as_bytes());
    let name = format!("teamagents-job-{}", &format!("{:x}", hasher.finalize())[..32]);
    use std::os::linux::net::SocketAddrExt;
    let std_addr =
        std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).map_err(|e| format!("socket addr: {e}"))?;
    Ok(tokio::net::unix::SocketAddr::from(std_addr))
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn boot_id() -> Result<String, String> {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|s| s.trim().to_string())
        .map_err(|e| format!("boot_id: {e}"))
}

/// /proc start ticks prove the recorded PID is still the same process.
pub fn start_ticks(pid: u32) -> Result<u64, String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| format!("proc stat {pid}: {e}"))?;
    let tail = text.rsplit_once(')').ok_or("invalid process stat")?.1;
    tail.split_whitespace()
        .nth(19)
        .ok_or_else(|| "invalid process start time".to_string())?
        .parse()
        .map_err(|e| format!("start ticks: {e}"))
}

/// Signal the process group the runner itself created — never a PID taken
/// from outside. Identity is re-verified before any signal (§6.2).
pub fn signal_group(journal: &Journal, signal: i32) -> Result<(), String> {
    let pid = journal.pid.ok_or("no verifiable process")?;
    if journal.boot_id != boot_id()? || journal.start_ticks != Some(start_ticks(pid)?) {
        return Err("process identity changed, refusing to signal".into());
    }
    let result = unsafe { libc::kill(-(pid as i32), signal) };
    if result != 0 {
        return Err(format!("signal group -{pid}: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

/// Write JSON atomically (tmp + rename + dir sync), matching the artifact
/// ordering rule: a partial journal is never observed (§4.3).
pub fn atomic_json<T: Serialize>(path: &std::path::Path, value: &T) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    let bytes = serde_json::to_vec(value).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&tmp, bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    {
        let file = std::fs::File::open(&tmp).map_err(|e| format!("open {}: {e}", tmp.display()))?;
        file.sync_all().map_err(|e| format!("sync {}: {e}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("rename {}: {e}", path.display()))?;
    if let Some(dir) = path.parent() {
        if let Ok(dir) = std::fs::File::open(dir) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

/// One coordinator per state root (§6.1): an OS file lock whose descriptor
/// is never inherited by tool children (std sets CLOEXEC).
pub fn state_lock(path: &std::path::Path) -> Result<std::fs::File, String> {
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("open lock {}: {e}", path.display()))?;
    lock.try_lock().map_err(|e| format!("state root already has a coordinator: {e}"))?;
    Ok(lock)
}
