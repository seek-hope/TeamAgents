//! The user's work surface (D-267): list every work a session carries, and open the
//! next one — `teamagents work_scopes [list]` / `teamagents work_scopes open --id ID [--attach INSTANCE]`.
//!
//! A session's work is anchored to a work (§8) and a settled work cannot be reopened, so
//! "what do I do next in this session?" had no answer a user could act on: `create_work`
//! is an ordinary user command in the protocol, the daemon forwards it, and until this
//! client existed nothing in the product sent it (`docs/ACCEPTANCE.md`'s known gap). The
//! runtime still opens no work by itself and a *model* still cannot open one — that
//! question stays the user's — so this is a client of an existing, user-owned capability,
//! not a new authority.

use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};

use super::exec::Client;

/// What the user asked the work surface to do.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkCommand {
    /// Read every work: id, status, deadline, limits, settled usage and what is attached.
    List,
    /// Open a work (`ACTIVE` from the start), optionally attached to an instance — which is what makes a later
    /// delegation charge to it (D-266) — and optionally carrying the user's own required checks (§8: only the
    /// user or the project bootstrap predefines them).
    Open { id: String, attach: Option<String>, required_checks: Vec<Json>, deadline: Option<f64> },
    /// Close a work the user names (D-341/D-344): a work whose ceiling or deadline is spent stays `ACTIVE`
    /// for ever, refusing new work and still listed; the instance its refusal parked has no path back, so
    /// before this the user's only way out was to abandon the session. The verb settles the work terminal
    /// (`CANCELLED`) and releases the instances that work's refusal parked.
    Cancel { id: String },
}

pub struct WorkOptions {
    /// The running session's socket (`<state root>/daemon.sock`).
    pub socket: PathBuf,
    pub command: WorkCommand,
    pub json_out: bool,
}

/// Exit codes, like the other surfaces: 0 carried out, 1 the session refused it, 2 usage or no session.
pub fn run(options: WorkOptions) -> i32 {
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

/// One work command, with no printing at all. Errors are (exit code, message).
pub fn execute(options: &WorkOptions) -> Result<Json, (i32, String)> {
    let mut client = connect(&options.socket)?;
    let session = json!({"session_id": client.session_id, "state_root": client.state_root});
    match &options.command {
        WorkCommand::List => {
            let view = client
                .call("work_scopes", json!({}))
                .map_err(|error| (1, format!("work_scopes: cannot read the work_scopes: {error}")))?;
            let work_scopes = view["work_scopes"].as_array().cloned().unwrap_or_default();
            Ok(
                json!({"session_id": session["session_id"], "state_root": session["state_root"], "work_scopes": work_scopes}),
            )
        }
        WorkCommand::Open { id, attach, required_checks, deadline } => {
            if id.is_empty() {
                return Err((2, "work_scopes open needs a non-empty --id".into()));
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
                .command(&format!("work-open-{id}"), "create_work", params)
                .map_err(|error| (1, format!("work_scopes open: the session refused it: {error}")))?;
            // D-283: the daemon unions the session's configured required checks into every work a
            // client opens (D-268), so what the work *got* is not what this client asked for. Report
            // the work's own `required_checks` from the existing `work_scopes` read (D-267) instead of
            // `required_checks.len()`, which printed `(0 required check(s))` for a work that carries
            // the session's check. No new read and no new surface: the same read `work_scopes list` uses.
            let view = client.call("work_scopes", json!({})).map_err(|error| {
                (
                    1,
                    format!(
                        "work_scopes open: {id} opened, but the work_scopes read could not report its checks: {error}"
                    ),
                )
            })?;
            let row = view["work_scopes"]
                .as_array()
                .and_then(|rows| rows.iter().find(|row| row["id"] == json!(id)))
                .ok_or_else(|| {
                    (1, format!("work_scopes open: {id} opened, but the work_scopes read does not carry it"))
                })?;
            let checks = row["limits"]["required_checks"].as_array().map(Vec::len).unwrap_or(0);
            Ok(json!({"session_id": session["session_id"], "state_root": session["state_root"],
                      "work_id": id, "attached": attach, "checks": checks, "result": result}))
        }
        WorkCommand::Cancel { id } => {
            if id.is_empty() {
                return Err((2, "work_scopes cancel needs a non-empty --id".into()));
            }
            // the work id is the command's own identity: a replayed cancel under the same id returns the
            // stored receipt, and a *different* work under a replayed id is refused by the receipt check
            let result = client
                .command(&format!("work-cancel-{id}"), "cancel_work", json!({"work_id": id}))
                .map_err(|error| (1, format!("work_scopes cancel: the session refused it: {error}")))?;
            Ok(json!({"session_id": session["session_id"], "state_root": session["state_root"],
                      "work_id": id, "result": result}))
        }
    }
}

fn connect(socket: &Path) -> Result<Client, (i32, String)> {
    Client::connect(socket).map_err(|error| {
        (
            2,
            format!(
                "work_scopes: {error}\nwork_scopes works on a running session; start one with `teamagents` (TUI) or \
                 `teamagents exec \"…\"`, or point --state-root at the session you mean"
            ),
        )
    })
}

/// The tokens a reader can weigh: the work's settled usage against its ceiling, when it has one. Both numbers
/// come straight from the record the `work_scopes` read serves (`limits.max_total_tokens`, `known_usage.total`).
fn budget(work: &Json) -> Option<String> {
    let max = work["limits"]["max_total_tokens"].as_u64()?;
    let used = work["known_usage"]["total"].as_u64().unwrap_or(0);
    Some(format!("{used}/{max}"))
}

/// D-285: why the *record* shows this work cannot accept a new request — or none, when it does not show it.
/// Only an `ACTIVE` work is presented as in force, so only an `ACTIVE` work can be misread; a settled one
/// already says so. The two conditions are the runtime's own gates, computed from the same fields the `work_scopes`
/// read carries: `known_usage.total >= limits.max_total_tokens` is A18's ceiling (the gate refuses any request
/// whose estimate is at least one token, and every real request has one) and `now > deadline` is A35's
/// `work_deadline_passed`. `unknown_usage` is deliberately not an input: the budget gate leaves it out of the
/// sum, so counting it would overstate the ceiling. A partly-used ceiling is *not* a refusal — whether the next
/// request fits depends on an estimate the client does not have, and guessing one would be the same dishonesty
/// this fixes.
fn refusals(work: &Json, now: f64) -> Vec<String> {
    if work["status"] != json!("ACTIVE") {
        return Vec::new();
    }
    let mut reasons = Vec::new();
    if let Some(deadline) = work["deadline"].as_f64() {
        if now > deadline {
            reasons.push("deadline passed".to_string());
        }
    }
    if let Some(max) = work["limits"]["max_total_tokens"].as_u64() {
        let used = work["known_usage"]["total"].as_u64().unwrap_or(0);
        if used >= max {
            reasons.push(format!("ceiling reached ({used}/{max})"));
        }
    }
    reasons
}

fn print_report(options: &WorkOptions, report: &Json) {
    if options.json_out {
        println!("{}", serde_json::to_string(report).unwrap_or_else(|_| "{}".into()));
        return;
    }
    match &options.command {
        WorkCommand::List => {
            let work_scopes = report["work_scopes"].as_array().cloned().unwrap_or_default();
            println!(
                "session {} ({}): {} work(s)",
                report["session_id"].as_str().unwrap_or(""),
                report["state_root"].as_str().unwrap_or(""),
                work_scopes.len()
            );
            let now = teamagents_core::models::now();
            for work in &work_scopes {
                let attached: Vec<String> = work["attached_instances"]
                    .as_array()
                    .map(|list| list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                let checks = work["limits"]["required_checks"].as_array().map(Vec::len).unwrap_or(0);
                let reasons = refusals(work, now);
                println!(
                    "  {}  {:<10} attached: {:<18} checks: {}{}{}{}",
                    work["id"].as_str().unwrap_or(""),
                    work["status"].as_str().unwrap_or(""),
                    if attached.is_empty() { "-".to_string() } else { attached.join(",") },
                    checks,
                    work["deadline"].as_f64().map(|d| format!("  deadline: {d:.0}")).unwrap_or_default(),
                    budget(work).map(|b| format!("  tokens: {b}")).unwrap_or_default(),
                    if reasons.is_empty() {
                        String::new()
                    } else {
                        format!("  cannot accept new work: {}", reasons.join("; "))
                    },
                );
            }
        }
        WorkCommand::Cancel { id } => {
            let released: Vec<String> = report["result"]["released"]
                .as_array()
                .map(|list| list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            println!(
                "cancelled work {}{}",
                report["work_id"].as_str().unwrap_or(id),
                if released.is_empty() { String::new() } else { format!("; released {}", released.join(", ")) },
            );
        }
        WorkCommand::Open { id, .. } => {
            println!(
                "opened work {} ({} required check(s)){}",
                report["work_id"].as_str().unwrap_or(id),
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
    fn the_record_decides_which_work_scopes_cannot_accept_work() {
        let now = 1_790_600_478.0;
        // `work-s-main`: 1,926,270 of a 2,000,000 ceiling with a deadline that has passed. The ceiling still
        // leaves 73,730 tokens, so only the deadline is a refusal — but the budget is still shown.
        let s_main = json!({"id": "work-s-main", "status": "ACTIVE", "deadline": 1_790_594_385.79,
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

        // `work-task5`: room under the ceiling and a live deadline — nothing the record shows
        let live = json!({"id": "work-task5", "status": "ACTIVE", "deadline": now + 2_000.0,
                          "limits": {"max_total_tokens": 12_000_000}, "known_usage": {"total": 654_248},
                          "unknown_usage": 0});
        assert!(refusals(&live, now).is_empty(), "a work with room is not marked");
        assert_eq!(budget(&live).as_deref(), Some("654248/12000000"));

        // a settled work is not presented as in force, so it needs no marker; a work with neither a ceiling
        // nor a deadline has nothing the record can show, and `unknown_usage` is not a charge (the gate
        // leaves it out of the sum)
        let settled = json!({"id": "work-task2", "status": "SUCCEEDED", "deadline": 1.0,
                             "limits": {"max_total_tokens": 10}, "known_usage": {"total": 10},
                             "unknown_usage": 7});
        assert!(refusals(&settled, now).is_empty());
        let bare = json!({"id": "bare", "status": "ACTIVE"});
        assert!(refusals(&bare, now).is_empty());
        assert_eq!(budget(&bare), None);
    }
}
