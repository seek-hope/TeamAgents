//! The state root's command runners, headless (D-250): what a state root still has running, and retiring the
//! ones whose work is over.
//!
//! One `teamagents jobs-runner <job dir>` process serves one shell command (§6.2) and by design outlives the
//! daemon, its member and the client. Current builds retire one themselves once its job settles (D-112) or
//! once the job directory is gone (D-153), so what a state root still carries is: a command genuinely in
//! flight (the design's live partner, §6.2/§6.3), a job whose outcome is deliberately held as
//! `OUTCOME_UNKNOWN`, or a runner left by an older build. ACCEPTANCE's known gap recorded what the user could
//! do about the last two: "a user can only retire it by reopening that session". This is the lever.
//!
//! The identity question the gap raised dissolves the way `daemon --stop`'s did (D-248): the job directory
//! holds `job.json`, whose token addresses the runner's abstract socket, so the lever **asks the runner**
//! rather than signalling a pid — there is no pid to record, guess or reuse, and the runner's own gate is the
//! safety: `shutdown` is refused while it has an active child (§6.2, `jobs/client.rs`), so a command that is
//! really running is never taken away by a listing tool.
//!
//! The *other* leftover is the one a settled command left behind: a service started with `&`, whose shell is
//! long gone (`runners stop --service --yes`, D-251). Nothing can be asked there — no runner is left to ask —
//! so it is a signal, and its identity rule is not A15's: `signal_group` re-verifies the recorded child, which
//! a settled job no longer has. What identifies the group instead is *when its members were born*
//! (`jobs::stop_leftover_group`: every member predates the job's own end, so a group id the kernel handed out
//! again cannot pass). A runner from a build whose protocol this one does not speak is reported `unreachable`,
//! and the documented fallback for it stays the per-pid `kill` (`docs/USER-GUIDE.md` §4).

use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::jobs::{self, Journal};

/// How long one probe of a runner may take before it counts as not answering. A live runner answers `status`
/// in microseconds; one that is wedged is reported as such instead of hanging the verb.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// What the user asked the runner surface to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnersCommand {
    /// What this state root still carries.
    List,
    /// Ask every runner of this state root to retire, or the one named by `--id`.
    ///
    /// With `service` the action is the *other* leftover: the process group a settled command left behind (a
    /// service the command started and did not wait for, D-41/D-112). That one is a signal, not a request, so it
    /// needs the caller's explicit `--yes`.
    Stop { job_id: Option<String>, service: bool },
}

pub struct RunnersOptions {
    /// The state root to look at. Deliberately not a socket: this works while no session runs, which is
    /// exactly when a leftover matters.
    pub state_root: PathBuf,
    pub command: RunnersCommand,
    pub json_out: bool,
    /// The caller's `--yes`: the service stop signals processes, so it is deliberate.
    pub confirmed: bool,
}

/// One job directory of a state root: the owning instance (`instances/<id>/jobs/<job>`), or `None` for a
/// driver whose state root *is* the session root (`<state root>/jobs/<job>`, the one-instance shape).
fn job_dirs(state_root: &Path) -> Vec<(Option<String>, PathBuf)> {
    let mut out = Vec::new();
    let mut push = |instance: Option<String>, dir: PathBuf| out.push((instance, dir));
    for (instance, jobs) in instance_job_dirs(state_root) {
        if let Ok(entries) = std::fs::read_dir(&jobs) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    push(instance.clone(), entry.path());
                }
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(state_root.join("jobs")) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                push(None, entry.path());
            }
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

fn instance_job_dirs(state_root: &Path) -> Vec<(Option<String>, PathBuf)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(state_root.join("instances")) else { return out };
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        out.push((Some(name), entry.path().join("jobs")));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The journal a job directory holds, or `None` when there is none (a directory that never got that far, or
/// one whose journal was removed) — the census's `dir-gone` sibling.
fn journal_of(dir: &Path) -> Option<Journal> {
    jobs::client::persisted_journal(dir)
}

/// Whether a runner answers on this job's socket. `Ok(true)` a live runner, `Ok(false)` nothing listening,
/// `Err` a job directory this build cannot address (no `job.json`, or a token it cannot read).
async fn answers(dir: &Path) -> Result<bool, String> {
    match tokio::time::timeout(PROBE_TIMEOUT, jobs::client::status(dir)).await {
        Ok(Ok(_journal)) => Ok(true),
        Ok(Err(reason)) => {
            // a connect failure is "no runner"; a refusal from a *reached* runner is not
            if reason.contains("connect runner") {
                Ok(false)
            } else {
                Err(reason)
            }
        }
        Err(_) => Err("the runner did not answer within 2s".into()),
    }
}

/// One row of the report: everything the lever knows about one job directory, and (for `stop`) what it did.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    None,
    Retire,
    Service,
}

async fn row(instance: Option<String>, dir: &Path, act: Act, table: &[jobs::Proc]) -> Json {
    let journal = journal_of(dir);
    let job_id = journal
        .as_ref()
        .map(|journal| journal.job_id.clone())
        .or_else(|| dir.file_name().map(|name| name.to_string_lossy().into_owned()))
        .unwrap_or_default();
    // how many processes are still in the group the command created: the census half of the service lever
    let service = journal
        .as_ref()
        .filter(|journal| journal.terminal() && journal.boot_id == jobs::boot_id().unwrap_or_default())
        .and_then(|journal| journal.pid)
        .map(|group| jobs::group_members(table, group).len());
    let reachable = answers(dir).await;
    let runner = match &reachable {
        Ok(true) => "live".to_string(),
        Ok(false) => "gone".to_string(),
        Err(_) => "unreachable".to_string(),
    };
    // The service stop does not need a runner at all (the leftover it stops is the one whose runner is usually
    // already gone), so it is decided before the reachability of the runner means anything.
    let outcome = match act {
        Act::None => None,
        Act::Service => Some(match journal.as_ref() {
            Some(journal) => match jobs::stop_leftover_group(journal, libc::SIGTERM) {
                Ok(0) => "no service left".to_string(),
                Ok(count) => format!("service stopped: {count} process(es)"),
                Err(reason) => format!("refused: {reason}"),
            },
            None => "refused: the job directory has no journal".to_string(),
        }),
        // the runner ask: only a runner that answers can be asked, and its own gate is the safety
        Act::Retire => match reachable {
            Ok(true) => match tokio::time::timeout(PROBE_TIMEOUT, jobs::client::shutdown(dir)).await {
                Ok(Ok(())) => Some("retired".to_string()),
                Ok(Err(reason)) => Some(format!("refused: {reason}")),
                Err(_) => Some("refused: the runner did not answer within 2s".to_string()),
            },
            Ok(false) => Some("no runner".to_string()),
            Err(reason) => Some(format!("unreachable: {reason}")),
        },
    };
    json!({
        "job_id": job_id,
        "instance": instance,
        "state": journal.as_ref().map(|journal| journal.state.clone()),
        "terminal": journal.as_ref().map(|journal| journal.terminal()),
        "runner": runner,
        "child_pid": journal.as_ref().and_then(|journal| journal.pid),
        "service": service,
        "finished_ms": journal.as_ref().and_then(|journal| journal.finished_ms),
        "exit_code": journal.as_ref().and_then(|journal| journal.exit_code),
        "outcome": outcome,
    })
}

/// One runner command, for the CLI: builds the current-thread runtime the job client needs. Errors are
/// (exit code, message).
pub fn execute(options: &RunnersOptions) -> Result<Json, (i32, String)> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| (2, format!("runners: {error}")))?;
    runtime.block_on(execute_async(options))
}

/// The async core, without a runtime of its own — what the CLI's `execute` wraps and what a caller that is
/// already inside a runtime (the integration tests) calls directly. No printing.
pub async fn execute_async(options: &RunnersOptions) -> Result<Json, (i32, String)> {
    // The live session's id, when one answers: the jobs belong to the *state root* (one session per root, A33),
    // and this field says whether a session is there to own them — `null` is the case the verb exists for.
    let session = match crate::v2::exec::Client::connect(&options.state_root.join("daemon.sock")) {
        Ok(client) => Json::String(client.session_id),
        Err(_) => Json::Null,
    };
    let dirs = job_dirs(&options.state_root);
    let (act, wanted) = match &options.command {
        RunnersCommand::List => (Act::None, None),
        RunnersCommand::Stop { job_id, service } => {
            if *service && !options.confirmed {
                return Err((
                    2,
                    "runners stop --service signals the process group a settled command left behind \
                     (a server started with `&`, for instance): add --yes to mean it"
                        .to_string(),
                ));
            }
            (if *service { Act::Service } else { Act::Retire }, job_id.clone())
        }
    };
    // one process-table scan for the whole listing (the acting path re-reads it and re-verifies)
    let table = jobs::proc_table();
    let mut rows = Vec::new();
    for (instance, dir) in dirs {
        if let Some(wanted) = &wanted {
            let named = journal_of(&dir).map(|journal| journal.job_id);
            let matches = named.as_deref() == Some(wanted.as_str())
                || dir.file_name().map(|name| name.to_string_lossy() == wanted.as_str()).unwrap_or(false);
            if !matches {
                continue;
            }
        }
        rows.push(row(instance, &dir, act, &table).await);
    }
    if let Some(wanted) = wanted {
        if rows.is_empty() {
            return Err((
                2,
                format!("runners: no job {wanted:?} in {} (see `teamagents runners`)", options.state_root.display()),
            ));
        }
    }
    Ok(json!({
        "session_id": session,
        "state_root": options.state_root.to_string_lossy(),
        "runners": rows,
    }))
}

/// Exit codes, documented in the help text: 0 the command was carried out, 2 usage or no such job. A runner
/// that refused (it has a child) is *reported*, not an error: the verb did what it could, per job.
pub fn run(options: RunnersOptions) -> i32 {
    match execute(&options) {
        Ok(report) => {
            print_report(&options, &report);
            0
        }
        Err((code, message)) => {
            eprintln!("{message}");
            code
        }
    }
}

fn print_report(options: &RunnersOptions, report: &Json) {
    if options.json_out {
        println!("{}", serde_json::to_string(report).unwrap_or_else(|_| "{}".into()));
        return;
    }
    let rows = report["runners"].as_array().cloned().unwrap_or_default();
    let root = report["state_root"].as_str().unwrap_or("");
    let session = match report["session_id"].as_str() {
        Some(id) => format!("session {id} is live"),
        None => "no session is running".to_string(),
    };
    println!("{root}: {} job(s), {session}", rows.len());
    for row in &rows {
        let state = row["state"].as_str().unwrap_or("-");
        let runner = row["runner"].as_str().unwrap_or("-");
        let child = match row["child_pid"].as_u64() {
            Some(pid) => format!("child {pid}"),
            None => "no child".to_string(),
        };
        let outcome = row["outcome"].as_str().map(|outcome| format!("  → {outcome}")).unwrap_or_default();
        let group = match row["service"].as_u64() {
            Some(0) => "no group left".to_string(),
            Some(count) => format!("group: {count} process(es)"),
            None => "group: unknown".to_string(),
        };
        println!(
            "  {}  {}  runner: {runner}  {child}  {group}  {}{outcome}",
            row["job_id"].as_str().unwrap_or(""),
            state,
            match row["instance"].as_str() {
                Some(instance) => format!("instance {instance}"),
                None => "session root".to_string(),
            },
        );
    }
    if rows.is_empty() {
        println!("  nothing to show: this state root has no job directory");
    }
}
