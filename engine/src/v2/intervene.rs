//! The user's intervention surface (§5.4), headless: list instances and tasks, and
//! apply the levers the design gives the user — pause, resume, terminate, cancel a
//! task (D-68).
//!
//! The TUI has always had these (`p`/`r`/`t`/`c`), and several documented workflows
//! end there: `exec` tells a stuck user to "resume it in the TUI instances panel (r)",
//! the operating notes say a task that can only wait is released by cancelling it in
//! the tasks panel, and D-65's honest ending for a model that stops talking is
//! exactly that cancellation. A headless or CI user has no panel, so those
//! instructions were unreachable for them. The mechanisms are the ordinary user
//! commands (`set_lifecycle`, `cancel_task`), which the control plane already gates
//! by identity — the user controls every transition, the system may only park
//! (§5.4) — so this is a client.
//!
//! Termination stays *deliberate* (§5.4, the TUI asks for a confirmation): it
//! requires `--yes` here too, because it retires the instance's workspace (a
//! directory with uncommitted or unmerged work is never deleted, only reported).

use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};

use super::exec::{resolve_prefix, Client};

/// What the user asked the intervention surface to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterventionCommand {
    /// The session's instances with their lifecycle and phase.
    ListInstances,
    /// Stop driving an instance at its next safe boundary (§5.4).
    Pause { id: String },
    /// Let a parked or paused instance run again.
    Resume { id: String },
    /// Retire an instance: its open tasks and derived grants are dealt with
    /// explicitly and its workspace is retired (§5.4) — hence `--yes`.
    Terminate { id: String },
    /// The session's tasks with their assignee, goal and status.
    ListTasks,
    /// Cancel one task: the only documented way to release a delegator whose
    /// assignee stopped without settling it (D-65).
    CancelTask { id: String },
}

pub struct InterventionOptions {
    /// The running session's socket (`<state root>/daemon.sock`).
    pub socket: PathBuf,
    pub command: InterventionCommand,
    /// Termination is destructive-adjacent: it needs the caller's explicit `--yes`.
    pub confirmed: bool,
    pub json_out: bool,
}

/// Exit codes, documented in the help text: 0 the command was carried out,
/// 1 the session refused it, 2 usage (bad flags, an unknown or ambiguous id,
/// a termination without `--yes`) or no session to talk to.
pub fn run(options: InterventionOptions) -> i32 {
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

/// One intervention command, with no printing at all. Errors are (exit code, message).
pub fn execute(options: &InterventionOptions) -> Result<Json, (i32, String)> {
    let mut client = connect(&options.socket)?;
    let session = json!({"session_id": client.session_id, "state_root": client.state_root});
    let instances = |client: &mut Client| -> Result<Vec<Json>, (i32, String)> {
        let checkpoint = client
            .call("checkpoint", json!({}))
            .map_err(|error| (1, format!("intervene: cannot read the instances: {error}")))?;
        Ok(checkpoint["snapshot"]["instances"].as_array().cloned().unwrap_or_default())
    };
    let tasks = |client: &mut Client| -> Result<Vec<Json>, (i32, String)> {
        let view = client
            .call("tasks", json!({}))
            .map_err(|error| (1, format!("intervene: cannot read the tasks: {error}")))?;
        Ok(view["tasks"].as_array().cloned().unwrap_or_default())
    };
    match &options.command {
        InterventionCommand::ListInstances => Ok(json!({
            "session_id": session["session_id"], "state_root": session["state_root"],
            "instances": instances(&mut client)?,
        })),
        InterventionCommand::ListTasks => Ok(json!({
            "session_id": session["session_id"], "state_root": session["state_root"],
            "tasks": tasks(&mut client)?,
        })),
        InterventionCommand::Pause { id }
        | InterventionCommand::Resume { id }
        | InterventionCommand::Terminate { id } => {
            let lifecycle = match options.command {
                InterventionCommand::Pause { .. } => "PAUSED",
                InterventionCommand::Resume { .. } => "ACTIVE",
                _ => "TERMINATED",
            };
            if lifecycle == "TERMINATED" && !options.confirmed {
                return Err((
                    2,
                    "terminating retires the instance and its workspace: add --yes to mean it \
                     (a directory with uncommitted or unmerged work is never deleted, only reported)"
                        .into(),
                ));
            }
            let rows = instances(&mut client)?;
            let row = resolve_prefix(&rows, "id", id, "instance", "teamagents instances")?;
            let resolved = row["id"].as_str().unwrap_or_default().to_string();
            let result = client
                .command(
                    &format!("lifecycle-{resolved}-{lifecycle}"),
                    "set_lifecycle",
                    json!({"instance_id": resolved, "lifecycle": lifecycle,
                           "reason": format!("{lifecycle} by the user (teamagents instances)")}),
                )
                .map_err(|error| (1, format!("intervene: the session refused it: {error}")))?;
            Ok(json!({
                "session_id": session["session_id"], "state_root": session["state_root"],
                "instance_id": resolved, "lifecycle": lifecycle, "instance": row, "result": result,
            }))
        }
        InterventionCommand::CancelTask { id } => {
            let rows = tasks(&mut client)?;
            let row = resolve_prefix(&rows, "id", id, "task", "teamagents tasks")?;
            let resolved = row["id"].as_str().unwrap_or_default().to_string();
            let result = client
                .command(
                    &format!("cancel-{resolved}"),
                    "cancel_task",
                    json!({"task_id": resolved, "reason": "cancelled by the user (teamagents tasks)"}),
                )
                .map_err(|error| (1, format!("intervene: the session refused it: {error}")))?;
            Ok(json!({
                "session_id": session["session_id"], "state_root": session["state_root"],
                "task_id": resolved, "task": row, "result": result,
            }))
        }
    }
}

fn connect(socket: &Path) -> Result<Client, (i32, String)> {
    Client::connect(socket).map_err(|error| {
        (
            2,
            format!(
                "intervene: {error}\nthis works on a running session; start one with `teamagents` (TUI) \
                 or `teamagents exec \"…\"`, or point --state-root at the session you mean"
            ),
        )
    })
}

// ------------------------------------------------------------------ rendering --

fn print_report(options: &InterventionOptions, report: &Json) {
    if options.json_out {
        println!("{}", serde_json::to_string(report).unwrap_or_else(|_| "{}".into()));
        return;
    }
    match &options.command {
        InterventionCommand::ListInstances => {
            let instances = report["instances"].as_array().cloned().unwrap_or_default();
            println!("session {}: {} instance(s)", report["session_id"].as_str().unwrap_or(""), instances.len());
            for instance in &instances {
                let lifecycle = instance["lifecycle"].as_str().unwrap_or("");
                // D-165: say *why* an instance is not running — the runtime's own words from the last
                // lifecycle transition (the park reason, or "PAUSED by the user …"). An ACTIVE member's last
                // transition says nothing a user needs, so the row stays as it was for those.
                let why = instance["reason"]
                    .as_str()
                    .filter(|_| lifecycle != "ACTIVE")
                    .map(|reason| format!("  — {reason}"))
                    .unwrap_or_default();
                println!(
                    "  {}  {} / {}{}{}",
                    instance["id"].as_str().unwrap_or(""),
                    lifecycle,
                    instance["phase"].as_str().unwrap_or(""),
                    // which model a member runs on (D-69): a team can span providers
                    match instance["model"].as_str().filter(|model| !model.is_empty()) {
                        Some(model) => format!("  · {model}"),
                        None => String::new(),
                    },
                    why
                );
            }
        }
        InterventionCommand::ListTasks => {
            let tasks = report["tasks"].as_array().cloned().unwrap_or_default();
            println!("session {}: {} task(s)", report["session_id"].as_str().unwrap_or(""), tasks.len());
            for task in &tasks {
                println!(
                    "  {}  {}  assignee {}  goal {}",
                    task["id"].as_str().unwrap_or(""),
                    task["status"].as_str().unwrap_or(""),
                    task["assignee"].as_str().unwrap_or(""),
                    task["goal_id"].as_str().unwrap_or("")
                );
            }
        }
        InterventionCommand::Pause { .. }
        | InterventionCommand::Resume { .. }
        | InterventionCommand::Terminate { .. } => {
            // the row as it is *after* the lever, in the same shape `instances` prints. It used to print the
            // new lifecycle beside the row resolved *before* the change, which read as a contradiction —
            // "PAUSED i-leader: ACTIVE / TOOLS_PENDING" (measured 2026-09-27, D-177). The phase is the one the
            // change was made at (a lifecycle change does not move the execution position); `instances` shows
            // where it settles.
            println!(
                "{}: {} / {}",
                report["instance_id"].as_str().unwrap_or(""),
                report["lifecycle"].as_str().unwrap_or(""),
                report["instance"]["phase"].as_str().unwrap_or("")
            );
            if report["lifecycle"] == json!("TERMINATED") {
                println!(
                    "its open tasks and derived grants were dealt with; the workspace is retired unless it holds work"
                );
            }
        }
        InterventionCommand::CancelTask { .. } => println!(
            "cancelled {} (was {}): a delegator waiting on it is released",
            report["task_id"].as_str().unwrap_or(""),
            report["task"]["status"].as_str().unwrap_or("")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::InterventionCommand;

    #[test]
    fn the_commands_are_distinct() {
        assert_ne!(InterventionCommand::ListInstances, InterventionCommand::ListTasks);
        assert_ne!(InterventionCommand::Pause { id: "i-1".into() }, InterventionCommand::Resume { id: "i-1".into() });
        assert_ne!(
            InterventionCommand::Resume { id: "i-1".into() },
            InterventionCommand::Terminate { id: "i-1".into() }
        );
    }
}
