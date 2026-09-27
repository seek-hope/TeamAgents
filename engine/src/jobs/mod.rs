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
use std::time::Duration;

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

/// How long a coordinator waits for a lock that is momentarily busy. A
/// `Command::spawn` (a shell-job runner, an MCP server, a hook) forks a child
/// that inherits the whole file-descriptor table; `CLOEXEC` closes the lock in
/// that child at exec, but until then the child's copy keeps the lock alive.
/// A restart that lands in that window must wait a moment, not fail: the kernel
/// still grants the lock to exactly one holder, so exclusivity is unchanged.
const LOCK_WAIT: Duration = Duration::from_secs(2);
const LOCK_STEP: Duration = Duration::from_millis(10);

/// One coordinator per state root (§6.1): an OS file lock whose descriptor is
/// never inherited by tool children (std sets CLOEXEC) beyond the instant
/// between `fork` and `exec`.
pub fn state_lock(path: &std::path::Path) -> Result<std::fs::File, String> {
    state_lock_waiting(path, LOCK_WAIT)
}

fn state_lock_waiting(path: &std::path::Path, wait: Duration) -> Result<std::fs::File, String> {
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("open lock {}: {e}", path.display()))?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            // a real holder (another coordinator) outlives the window
            Err(error) if std::time::Instant::now() >= deadline => {
                return Err(format!("state root already has a coordinator: {error}"));
            }
            Err(std::fs::TryLockError::WouldBlock) => std::thread::sleep(LOCK_STEP),
            // a filesystem that cannot lock at all is a real error, not busy
            Err(error) => return Err(format!("cannot lock {}: {error}", path.display())),
        }
    }
}

/// One process of the machine's table, as the group identity check reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    /// The process group (`/proc/<pid>/stat`'s pgrp): what a job's own group is keyed by.
    pub group: u32,
    /// When the process started, in the same clock as the journal's `*_ms` fields (wall clock ms).
    pub start_ms: u64,
}

/// The machine's process table, from `/proc` — one scan, so a listing of many jobs costs one pass.
pub fn proc_table() -> Vec<Proc> {
    let boot_ms = boot_epoch_ms();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else { return out };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else { continue };
        let Ok(text) = std::fs::read_to_string(entry.path().join("stat")) else { continue };
        let Some(tail) = text.rsplit_once(')').map(|(_, tail)| tail) else { continue };
        let fields: Vec<&str> = tail.split_whitespace().collect();
        // after the command name: state(0) ppid(1) pgrp(2) session(3) … starttime(19)
        let (Some(group), Some(ticks)) = (
            fields.get(2).and_then(|field| field.parse::<u32>().ok()),
            fields.get(19).and_then(|field| field.parse::<u64>().ok()),
        ) else {
            continue;
        };
        out.push(Proc { pid, group, start_ms: boot_ms + ticks * 1000 / clock_ticks() });
    }
    out
}

/// The live members of the group `group`, in pid order.
pub fn group_members(table: &[Proc], group: u32) -> Vec<Proc> {
    let mut members: Vec<Proc> = table.iter().copied().filter(|proc_| proc_.group == group).collect();
    members.sort_by_key(|proc_| proc_.pid);
    members
}

fn clock_ticks() -> u64 {
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 {
        ticks as u64
    } else {
        100
    }
}

/// The wall-clock instant of boot, so a `/proc` start tick can be compared with the journal's `*_ms` fields.
fn boot_epoch_ms() -> u64 {
    let uptime_s = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|text| text.split_whitespace().next().and_then(|first| first.parse::<f64>().ok()))
        .unwrap_or(0.0);
    now_ms().saturating_sub((uptime_s * 1000.0) as u64)
}

/// Clock slack between a runner's `now_ms()` and a `/proc` start time: the two are read microseconds apart, and
/// the spawn happens after `started_ms` is written, so only the *upper* bound below is used for identity.
const SERVICE_SLACK_MS: u64 = 1000;

/// Stop the process group a *settled* command left behind — the service a command started and did not wait for
/// (DESIGN §9's explicit-only cleanup, D-41/D-112).
///
/// The identity question is different from `signal_group`'s (A15): that one re-verifies the recorded child, and
/// the direct child of a settled job is gone by definition — the code that does have a live child says why it can
/// trust the group id at all ("the direct child is not reaped yet, so its group ID cannot be reused",
/// `tools.rs`). Once the child *is* reaped the kernel may hand its pid out again, and a new process that becomes
/// its own group leader would wear the same group id. What rules that out here is **when the members were
/// born**: a member of the reused group is newer than the moment the job ended, while every process the command
/// left behind was born while the command ran (`started_ms` … `finished_ms`). So the rule is: a terminal
/// journal, the same boot, at least one live member of the group, and **every** member born at or before the
/// job's end — then one `kill(-group)` reaches the command's own leftovers and nothing else.
///
/// Returns how many members were signalled; `Ok(0)` means the group is gone (nothing left), never a refusal.
pub fn stop_leftover_group(journal: &Journal, signal: i32) -> Result<usize, String> {
    let pid = journal.pid.ok_or("no recorded process group")?;
    if !journal.terminal() {
        return Err(format!(
            "job {} is not settled ({}): its command may still be running, so its group is not a leftover",
            journal.job_id, journal.state
        ));
    }
    if journal.boot_id != boot_id()? {
        return Err("the journal is from another boot: its process group id means nothing here".into());
    }
    let finished = journal.finished_ms.ok_or("the journal has no end time to bound the group by")?;
    let members = group_members(&proc_table(), pid);
    if members.is_empty() {
        return Ok(0);
    }
    for member in &members {
        if member.start_ms > finished.saturating_add(SERVICE_SLACK_MS) {
            return Err(format!(
                "process {} started after this job ended ({} > {}): the group id was reused, refusing to signal",
                member.pid, member.start_ms, finished
            ));
        }
    }
    let result = unsafe { libc::kill(-(pid as i32), signal) };
    if result != 0 {
        return Err(format!("signal group -{pid}: {}", std::io::Error::last_os_error()));
    }
    Ok(members.len())
}

#[cfg(test)]
mod tests {
    use super::{state_lock, state_lock_waiting};
    use std::time::{Duration, Instant};

    /// D-251's identity rule, measured with a forged journal rather than a pid that really was recycled (which
    /// no test can arrange): the group id a settled job recorded is *refused* when the live members of that
    /// group were born after the job ended — exactly the shape a recycled pid would have, and the reason the
    /// rule is about birth times and not about the id alone.
    #[test]
    fn a_group_whose_members_are_newer_than_the_job_is_never_signalled() {
        use super::{boot_id, group_members, now_ms, proc_table, start_ticks, stop_leftover_group, Journal};
        use std::os::unix::process::CommandExt;
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .process_group(0)
            .spawn()
            .expect("spawn a process group of its own");
        let pid = child.id();
        let journal = Journal {
            job_id: "op-forged".into(),
            state: "SUCCEEDED".into(),
            command_hash: "forged".into(),
            pid: Some(pid),
            start_ticks: Some(start_ticks(pid).expect("start ticks")),
            boot_id: boot_id().expect("boot id"),
            exit_code: Some(0),
            signal: None,
            // the job ended half a minute ago; the process wearing its group id started just now
            started_ms: Some(now_ms().saturating_sub(60_000)),
            finished_ms: Some(now_ms().saturating_sub(30_000)),
            starts: 1,
            cancel_saved: false,
        };
        let refused = stop_leftover_group(&journal, libc::SIGTERM).expect_err("a group born after the job: refuse");
        assert!(refused.contains("reused"), "the reason names the shape: {refused}");
        assert_eq!(group_members(&proc_table(), pid).len(), 1, "nothing was signalled: the stranger is alive");
        let _ = child.kill();
        let _ = child.wait();
    }

    fn temp_path(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ta-state-lock-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("coordinator.lock")
    }

    /// The window between `fork` and `exec` keeps an inherited lock alive for an
    /// instant: a restart that lands in it waits and then succeeds, instead of
    /// reporting a coordinator that is not really there.
    #[test]
    fn a_momentarily_held_lock_is_absorbed() {
        let path = temp_path("absorb");
        let holder = state_lock(&path).unwrap();
        // The clock starts *before* the releaser exists: under load the main thread can be starved for longer
        // than the holder's 60 ms between the spawn and the first measurement, and the test then reports "it
        // must actually wait" for a wait that really happened (reproduced by sleeping 70 ms in that window,
        // D-138). Measured from here, a lock that is *stolen* still returns within `LOCK_STEP` — well inside
        // the 40 ms the assertion allows — so the guard keeps its meaning.
        let started = Instant::now();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            drop(holder);
        });
        let taken = state_lock_waiting(&path, Duration::from_secs(2)).expect("the window closes");
        assert!(started.elapsed() >= Duration::from_millis(40), "it must actually wait, not steal the lock");
        drop(taken);
        release.join().unwrap();
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A real second coordinator keeps the lock: the wait is bounded and the
    /// refusal still names the state-root rule (A33).
    #[test]
    fn a_live_holder_is_refused_after_the_wait() {
        let path = temp_path("refuse");
        let _holder = state_lock(&path).unwrap();
        let error = state_lock_waiting(&path, Duration::from_millis(80)).expect_err("a live holder wins");
        assert!(error.contains("state root already has a coordinator"), "{error}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// The default wait is short enough not to stall a real refusal and long
    /// enough to cover a fork/exec window.
    #[test]
    fn the_default_wait_is_bounded() {
        assert!(super::LOCK_WAIT >= Duration::from_millis(500) && super::LOCK_WAIT <= Duration::from_secs(5));
        assert!(super::LOCK_STEP < super::LOCK_WAIT);
    }

    /// D-116: the runner's tick is the whole CPU cost of a running job (at 10 ms it measured 1.80 % of a core
    /// per command, because *every* command pays it for as long as it runs). It must stay coarse enough to be
    /// cheap and fine enough that each latency it buys stays far below what a person notices.
    #[test]
    fn the_runner_tick_trades_cpu_for_a_bounded_latency() {
        use super::runner::{CANCEL_ESCALATION_MS, TICK};
        // these are constant comparisons, so they are checked when this file is compiled rather than when the
        // test runs (clippy asks for exactly this shape)
        const { assert!(TICK.as_millis() >= 25, "a finer tick than 25 ms is CPU the loop cannot justify") };
        const { assert!(TICK.as_millis() <= 200, "a coarser tick than 200 ms starts to be felt") };
        const { assert!(CANCEL_ESCALATION_MS <= 1000, "the escalation budget stays under a second") };
        const {
            assert!(
                CANCEL_ESCALATION_MS + TICK.as_millis() as u64 <= 1000,
                "TERM→KILL lands within a second even on the slowest tick"
            )
        };
    }

    /// D-153: an *idle* runner (no child, no cancel in flight) has nothing a timer can notice — its tick's
    /// three jobs all need a child — so its cadence only has to see its job directory disappear. It must stay
    /// far coarser than the running-job tick, because a leaked runner used to pay that tick forever (measured
    /// 2026-09-26: 0.40 % of a core per process, 1,397 of them on one machine), and it must still be bounded,
    /// because a job directory that is gone means no client can resolve the runner's socket any more. The
    /// measurement itself is `review/runner_cost.py`'s idle window.
    #[test]
    fn an_idle_runner_checks_rarely_but_never_waits_forever() {
        use super::runner::IDLE_TICK;
        const { assert!(IDLE_TICK.as_secs() >= 5, "an idle runner must not keep a running-job cadence") };
        const { assert!(IDLE_TICK.as_secs() <= 120, "an unreachable runner should not linger for minutes") };
    }
}
