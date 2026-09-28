//! The user's goal surface (D-267): list every goal a session carries, and open the
//! next one — `teamagents goals [list]` / `teamagents goals open --id ID [--attach INSTANCE]`.
//!
//! A session's work is anchored to a goal (§8) and a settled goal cannot be reopened, so
//! "what do I do next in this session?" had no answer a user could act on: `create_goal`
//! is an ordinary user command in the protocol, the daemon forwards it, and until this
//! client existed nothing in the product sent it (`docs/ACCEPTANCE.md`'s known gap). The
//! runtime still opens no goal by itself and a *model* still cannot open one — that
//! question stays the user's — so this is a client of an existing, user-owned capability,
//! not a new authority.

use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};

use super::exec::Client;

/// What the user asked the goal surface to do.
#[derive(Debug, Clone, PartialEq)]
pub enum GoalCommand {
    /// Read every goal: id, status, deadline, limits, settled usage and what is attached.
    List,
    /// Open a goal (`ACTIVE` from the start), optionally attached to an instance — which is what makes a later
    /// delegation charge to it (D-266) — and optionally carrying the user's own required checks (§8: only the
    /// user or the project bootstrap predefines them).
    Open { id: String, attach: Option<String>, required_checks: Vec<Json>, deadline: Option<f64> },
}

pub struct GoalOptions {
    /// The running session's socket (`<state root>/daemon.sock`).
    pub socket: PathBuf,
    pub command: GoalCommand,
    pub json_out: bool,
}

/// Exit codes, like the other surfaces: 0 carried out, 1 the session refused it, 2 usage or no session.
pub fn run(options: GoalOptions) -> i32 {
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

/// One goal command, with no printing at all. Errors are (exit code, message).
pub fn execute(options: &GoalOptions) -> Result<Json, (i32, String)> {
    let mut client = connect(&options.socket)?;
    let session = json!({"session_id": client.session_id, "state_root": client.state_root});
    match &options.command {
        GoalCommand::List => {
            let view = client
                .call("goals", json!({}))
                .map_err(|error| (1, format!("goals: cannot read the goals: {error}")))?;
            let goals = view["goals"].as_array().cloned().unwrap_or_default();
            Ok(json!({"session_id": session["session_id"], "state_root": session["state_root"], "goals": goals}))
        }
        GoalCommand::Open { id, attach, required_checks, deadline } => {
            if id.is_empty() {
                return Err((2, "goals open needs a non-empty --id".into()));
            }
            let mut params = json!({"id": id});
            if let Some(instance) = attach {
                params["instance_id"] = json!(instance);
            }
            if let Some(deadline) = deadline {
                params["deadline"] = json!(deadline);
            }
            if !required_checks.is_empty() {
                params["limits"] = json!({"required_checks": required_checks});
            }
            let result = client
                .command(&format!("goal-open-{id}"), "create_goal", params)
                .map_err(|error| (1, format!("goals open: the session refused it: {error}")))?;
            Ok(json!({"session_id": session["session_id"], "state_root": session["state_root"],
                      "goal_id": id, "attached": attach, "checks": required_checks.len(), "result": result}))
        }
    }
}

fn connect(socket: &Path) -> Result<Client, (i32, String)> {
    Client::connect(socket).map_err(|error| {
        (
            2,
            format!(
                "goals: {error}\ngoals works on a running session; start one with `teamagents` (TUI) or \
                 `teamagents exec \"…\"`, or point --state-root at the session you mean"
            ),
        )
    })
}

fn print_report(options: &GoalOptions, report: &Json) {
    if options.json_out {
        println!("{}", serde_json::to_string(report).unwrap_or_else(|_| "{}".into()));
        return;
    }
    match &options.command {
        GoalCommand::List => {
            let goals = report["goals"].as_array().cloned().unwrap_or_default();
            println!(
                "session {} ({}): {} goal(s)",
                report["session_id"].as_str().unwrap_or(""),
                report["state_root"].as_str().unwrap_or(""),
                goals.len()
            );
            for goal in &goals {
                let attached: Vec<String> = goal["attached_instances"]
                    .as_array()
                    .map(|list| list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                let checks = goal["limits"]["required_checks"].as_array().map(Vec::len).unwrap_or(0);
                println!(
                    "  {}  {:<10} attached: {:<18} checks: {}{}",
                    goal["id"].as_str().unwrap_or(""),
                    goal["status"].as_str().unwrap_or(""),
                    if attached.is_empty() { "-".to_string() } else { attached.join(",") },
                    checks,
                    goal["deadline"].as_f64().map(|d| format!("  deadline: {d:.0}")).unwrap_or_default(),
                );
            }
        }
        GoalCommand::Open { id, .. } => {
            println!(
                "opened goal {} ({} required check(s)){}",
                report["goal_id"].as_str().unwrap_or(id),
                report["checks"].as_u64().unwrap_or(0),
                report["attached"].as_str().map(|i| format!(", attached to {i}")).unwrap_or_default(),
            );
        }
    }
}
