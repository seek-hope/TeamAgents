//! The user's approval surface (D-67): list the approvals a session is waiting on
//! and decide them — `teamagents approvals [approve|deny] --id …`.
//!
//! A session in `approved_scope` (the default) parks an out-of-scope operation on a
//! user decision, and the design names the TUI as that surface (§5.4/§9). A
//! *headless* run has nobody at a TUI: `exec` reports the approval and exits 3, and
//! until this surface existed the only ways on were to start the TUI or to run the
//! session in `--full-auto`. The mechanism itself was never missing — the daemon's
//! `approvals` read carries the id, the operation and a preview, and
//! `approve`/`deny` are ordinary user commands, gated by the same
//! argument-hash-bound decision the runtime verifies before it dispatches
//! (`NoEffectBeforeApproval`, A25) — so this is a client, not a new authority.

use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};

use super::exec::{resolve_prefix, Client};

/// What the user asked the approval surface to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalCommand {
    /// Read the pending approvals (the TUI's approvals box, headless).
    List,
    /// Approve one, once: the decision is bound to that operation and its argument
    /// hash, so a modified call needs a new decision (§6.2).
    Approve { id: String },
    /// Deny one: the operation fails closed and the model sees the refusal.
    Deny { id: String },
}

pub struct ApprovalOptions {
    /// The running session's socket (`<state root>/daemon.sock`).
    pub socket: PathBuf,
    pub command: ApprovalCommand,
    pub json_out: bool,
}

/// Exit codes, documented in the help text: 0 the command was carried out,
/// 1 the session refused it, 2 usage (bad flags, an unknown or ambiguous id) or no
/// session to talk to.
pub fn run(options: ApprovalOptions) -> i32 {
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

/// One approval command, with no printing at all. Errors are (exit code, message).
pub fn execute(options: &ApprovalOptions) -> Result<Json, (i32, String)> {
    let mut client = connect(&options.socket)?;
    let session = json!({"session_id": client.session_id, "state_root": client.state_root});
    let pending = |client: &mut Client| -> Result<Vec<Json>, (i32, String)> {
        let view = client
            .call("approvals", json!({}))
            .map_err(|error| (1, format!("approvals: cannot read the approvals: {error}")))?;
        Ok(view["approvals"].as_array().cloned().unwrap_or_default())
    };
    match &options.command {
        ApprovalCommand::List => Ok(json!({
            "session_id": session["session_id"], "state_root": session["state_root"],
            "approvals": pending(&mut client)?,
        })),
        ApprovalCommand::Approve { id } | ApprovalCommand::Deny { id } => {
            let method = if matches!(options.command, ApprovalCommand::Approve { .. }) { "approve" } else { "deny" };
            // the id must name a *pending* approval: a typo is a client error here,
            // not a refusal the control plane has to guess at
            let rows = pending(&mut client)?;
            let row = resolve_prefix(&rows, "id", id, "approval", "teamagents approvals")?;
            let resolved = row["id"].as_str().unwrap_or_default().to_string();
            let result = client
                .command(&format!("decide-{resolved}-{method}"), method, json!({"approval_id": resolved}))
                .map_err(|error| (1, format!("approvals: the session refused the decision: {error}")))?;
            Ok(json!({
                "session_id": session["session_id"], "state_root": session["state_root"],
                "approval_id": resolved, "decision": method, "approval": row, "result": result,
            }))
        }
    }
}

pub(crate) fn connect(socket: &Path) -> Result<Client, (i32, String)> {
    Client::connect(socket).map_err(|error| {
        (
            2,
            format!(
                "approvals: {error}\napprovals works on a running session; start one with `teamagents` \
                 (TUI) or `teamagents exec \"…\"`, or point --state-root at the session you mean"
            ),
        )
    })
}

// ------------------------------------------------------------------ rendering --

fn print_report(options: &ApprovalOptions, report: &Json) {
    if options.json_out {
        println!("{}", serde_json::to_string(report).unwrap_or_else(|_| "{}".into()));
        return;
    }
    match &options.command {
        ApprovalCommand::List => {
            let approvals = report["approvals"].as_array().cloned().unwrap_or_default();
            println!(
                "session {} ({}): {} pending approval(s)",
                report["session_id"].as_str().unwrap_or(""),
                report["state_root"].as_str().unwrap_or(""),
                approvals.len()
            );
            for approval in &approvals {
                println!(
                    "  {}  {}  {}",
                    approval["id"].as_str().unwrap_or(""),
                    approval["tool"].as_str().unwrap_or(""),
                    approval["preview"].as_str().unwrap_or("")
                );
            }
            if !approvals.is_empty() {
                println!(
                    "decide one with `teamagents approvals approve --id <id>` or `… deny --id <id>` \
                     (the decision is bound to that call and its arguments)"
                );
            }
        }
        ApprovalCommand::Approve { .. } | ApprovalCommand::Deny { .. } => {
            let approval = &report["approval"];
            println!(
                "{} {}  ({}: {})",
                report["decision"].as_str().unwrap_or("decided"),
                report["approval_id"].as_str().unwrap_or(""),
                approval["tool"].as_str().unwrap_or(""),
                approval["preview"].as_str().unwrap_or("")
            );
            if report["decision"] == json!("deny") {
                println!("the operation fails closed and the model is told the user denied it");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ApprovalCommand;

    #[test]
    fn the_commands_are_distinct() {
        assert_ne!(ApprovalCommand::List, ApprovalCommand::Approve { id: "ap-1".into() });
        assert_ne!(ApprovalCommand::Approve { id: "ap-1".into() }, ApprovalCommand::Deny { id: "ap-1".into() });
    }
}
