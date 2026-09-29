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
    /// Close a goal the user names (D-341/D-344): a goal whose ceiling or deadline is spent stays `ACTIVE`
    /// for ever, refusing new work and still listed; the instance its refusal parked has no path back, so
    /// before this the user's only way out was to abandon the session. The verb settles the goal terminal
    /// (`CANCELLED`) and releases the instances that goal's refusal parked.
    Cancel { id: String },
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
            // D-283: the daemon unions the session's configured required checks into every goal a
            // client opens (D-268), so what the goal *got* is not what this client asked for. Report
            // the goal's own `required_checks` from the existing `goals` read (D-267) instead of
            // `required_checks.len()`, which printed `(0 required check(s))` for a goal that carries
            // the session's check. No new read and no new surface: the same read `goals list` uses.
            let view = client.call("goals", json!({})).map_err(|error| {
                (1, format!("goals open: {id} opened, but the goals read could not report its checks: {error}"))
            })?;
            let row = view["goals"]
                .as_array()
                .and_then(|rows| rows.iter().find(|row| row["id"] == json!(id)))
                .ok_or_else(|| (1, format!("goals open: {id} opened, but the goals read does not carry it")))?;
            let checks = row["limits"]["required_checks"].as_array().map(Vec::len).unwrap_or(0);
            Ok(json!({"session_id": session["session_id"], "state_root": session["state_root"],
                      "goal_id": id, "attached": attach, "checks": checks, "result": result}))
        }
        GoalCommand::Cancel { id } => {
            if id.is_empty() {
                return Err((2, "goals cancel needs a non-empty --id".into()));
            }
            // the goal id is the command's own identity: a replayed cancel under the same id returns the
            // stored receipt, and a *different* goal under a replayed id is refused by the receipt check
            let result = client
                .command(&format!("goal-cancel-{id}"), "cancel_goal", json!({"goal_id": id}))
                .map_err(|error| (1, format!("goals cancel: the session refused it: {error}")))?;
            Ok(json!({"session_id": session["session_id"], "state_root": session["state_root"],
                      "goal_id": id, "result": result}))
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

/// The tokens a reader can weigh: the goal's settled usage against its ceiling, when it has one. Both numbers
/// come straight from the record the `goals` read serves (`limits.max_total_tokens`, `known_usage.total`).
fn budget(goal: &Json) -> Option<String> {
    let max = goal["limits"]["max_total_tokens"].as_u64()?;
    let used = goal["known_usage"]["total"].as_u64().unwrap_or(0);
    Some(format!("{used}/{max}"))
}

/// D-285: why the *record* shows this goal cannot accept a new request — or none, when it does not show it.
/// Only an `ACTIVE` goal is presented as in force, so only an `ACTIVE` goal can be misread; a settled one
/// already says so. The two conditions are the runtime's own gates, computed from the same fields the `goals`
/// read carries: `known_usage.total >= limits.max_total_tokens` is A18's ceiling (the gate refuses any request
/// whose estimate is at least one token, and every real request has one) and `now > deadline` is A35's
/// `goal_deadline_passed`. `unknown_usage` is deliberately not an input: the budget gate leaves it out of the
/// sum, so counting it would overstate the ceiling. A partly-used ceiling is *not* a refusal — whether the next
/// request fits depends on an estimate the client does not have, and guessing one would be the same dishonesty
/// this fixes.
fn refusals(goal: &Json, now: f64) -> Vec<String> {
    if goal["status"] != json!("ACTIVE") {
        return Vec::new();
    }
    let mut reasons = Vec::new();
    if let Some(deadline) = goal["deadline"].as_f64() {
        if now > deadline {
            reasons.push("deadline passed".to_string());
        }
    }
    if let Some(max) = goal["limits"]["max_total_tokens"].as_u64() {
        let used = goal["known_usage"]["total"].as_u64().unwrap_or(0);
        if used >= max {
            reasons.push(format!("ceiling reached ({used}/{max})"));
        }
    }
    reasons
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
            let now = teamagents_core::models::now();
            for goal in &goals {
                let attached: Vec<String> = goal["attached_instances"]
                    .as_array()
                    .map(|list| list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                let checks = goal["limits"]["required_checks"].as_array().map(Vec::len).unwrap_or(0);
                let reasons = refusals(goal, now);
                println!(
                    "  {}  {:<10} attached: {:<18} checks: {}{}{}{}",
                    goal["id"].as_str().unwrap_or(""),
                    goal["status"].as_str().unwrap_or(""),
                    if attached.is_empty() { "-".to_string() } else { attached.join(",") },
                    checks,
                    goal["deadline"].as_f64().map(|d| format!("  deadline: {d:.0}")).unwrap_or_default(),
                    budget(goal).map(|b| format!("  tokens: {b}")).unwrap_or_default(),
                    if reasons.is_empty() {
                        String::new()
                    } else {
                        format!("  cannot accept new work: {}", reasons.join("; "))
                    },
                );
            }
        }
        GoalCommand::Cancel { id } => {
            let released: Vec<String> = report["result"]["released"]
                .as_array()
                .map(|list| list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            println!(
                "cancelled goal {}{}",
                report["goal_id"].as_str().unwrap_or(id),
                if released.is_empty() { String::new() } else { format!("; released {}", released.join(", ")) },
            );
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

#[cfg(test)]
mod tests {
    use super::*;

    /// D-284, on the numbers the operator's own session carried (D-278/D-280): what a reader is shown is
    /// computed from the record's own fields, and the two refusals are the runtime's two gates.
    #[test]
    fn the_record_decides_which_goals_cannot_accept_work() {
        let now = 1_790_600_478.0;
        // `goal-s-main`: 1,926,270 of a 2,000,000 ceiling with a deadline that has passed. The ceiling still
        // leaves 73,730 tokens, so only the deadline is a refusal — but the budget is still shown.
        let s_main = json!({"id": "goal-s-main", "status": "ACTIVE", "deadline": 1_790_594_385.79,
                            "limits": {"max_total_tokens": 2_000_000},
                            "known_usage": {"prompt": 1_902_081, "completion": 24_189, "total": 1_926_270},
                            "unknown_usage": 0});
        assert_eq!(refusals(&s_main, now), vec!["deadline passed"]);
        assert_eq!(budget(&s_main).as_deref(), Some("1926270/2000000"));

        // a ceiling with no room for even a one-token request: A18 refuses every real request
        let at_ceiling = json!({"id": "at-ceiling", "status": "ACTIVE", "deadline": null,
                                "limits": {"max_total_tokens": 10}, "known_usage": {"total": 10},
                                "unknown_usage": 0});
        assert_eq!(refusals(&at_ceiling, now), vec!["ceiling reached (10/10)"]);

        // `goal-task5`: room under the ceiling and a live deadline — nothing the record shows
        let live = json!({"id": "goal-task5", "status": "ACTIVE", "deadline": now + 2_000.0,
                          "limits": {"max_total_tokens": 12_000_000}, "known_usage": {"total": 654_248},
                          "unknown_usage": 0});
        assert!(refusals(&live, now).is_empty(), "a goal with room is not marked");
        assert_eq!(budget(&live).as_deref(), Some("654248/12000000"));

        // a settled goal is not presented as in force, so it needs no marker; a goal with neither a ceiling
        // nor a deadline has nothing the record can show, and `unknown_usage` is not a charge (the gate
        // leaves it out of the sum)
        let settled = json!({"id": "goal-task2", "status": "SUCCEEDED", "deadline": 1.0,
                             "limits": {"max_total_tokens": 10}, "known_usage": {"total": 10},
                             "unknown_usage": 7});
        assert!(refusals(&settled, now).is_empty());
        let bare = json!({"id": "bare", "status": "ACTIVE"});
        assert!(refusals(&bare, now).is_empty());
        assert_eq!(budget(&bare), None);
    }
}
