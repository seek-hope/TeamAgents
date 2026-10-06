//! The user's authority surface (§5.1, D-61): list the session's grants, issue
//! a scoped one and revoke one by id.
//!
//! The design makes the user the root of authority and gives the Leader
//! management by default, but a *spawned* worker holds no `shell@workspace`
//! (§5.1), and until this surface existed nothing in the product could hand it
//! one: `issue_grant`/`revoke_grant` had no caller outside the tests and the
//! evaluation harness, so the capability boundary was real but unreachable. This
//! is a client of the same daemon socket the TUI and `exec` use — it never opens
//! the database itself, so grants linearize with driver dispatch on the
//! single-writer worker exactly like every other business command (§9).
//!
//! Two client-side guards stand between a human and the control plane, because
//! the control plane stores any string a caller sends:
//! * an action outside the runtime's vocabulary, or an action/scope pair no
//!   check ever asks about, is refused — such a row would authorize nothing
//!   (`control::capability::authorizes_something`);
//! * a subject that is not an instance of this session is a *warning*, not a
//!   refusal: instance ids are chosen by the spawner, so a user may legitimately
//!   grant ahead of a spawn — and may equally have made a typo.

use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};
use teamagents_core::v2::capability;

use super::exec::Client;

/// What the user asked the authority surface to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorityCommand {
    /// Read the session's grants (with the ids a revoke needs).
    List,
    /// Issue a scoped grant for one subject.
    Grant { subject: String, action: String, scope: String, parent: Option<String> },
    /// Revoke one grant by id (an unambiguous prefix is enough) and its subtree.
    Revoke { grant: String },
}

pub struct AuthorityOptions {
    /// The running session's socket (`<state root>/daemon.sock`).
    pub socket: PathBuf,
    pub command: AuthorityCommand,
    pub json_out: bool,
}

/// Exit codes, documented in the help text: 0 the command was carried out,
/// 1 the session refused it, 2 usage (bad flags or a pair nothing checks) or no
/// session to talk to.
pub fn run(options: AuthorityOptions) -> i32 {
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

/// One authority command, with no printing at all. Errors are (exit code, message).
pub fn execute(options: &AuthorityOptions) -> Result<Json, (i32, String)> {
    let mut client = connect(&options.socket)?;
    let session = json!({"session_id": client.session_id, "state_root": client.state_root});
    let report = match &options.command {
        AuthorityCommand::List => {
            let view = read_grants(&mut client)?;
            // The authority surface lists the *subjects* too: a grant names an
            // instance, and the id is what `grant --subject` takes. The TUI shows
            // the same rows; a headless user has no other way to read them.
            let checkpoint = client
                .call("checkpoint", json!({}))
                .map_err(|error| (1, format!("authority: cannot read the instances: {error}")))?;
            let report = json!({"session_id": session["session_id"], "state_root": session["state_root"],
                   "revision": view["revision"], "instances": checkpoint["snapshot"]["instances"],
                   "grants": view["grants"]});
            let gaps = capability_gaps(&report);
            let mut report = report;
            report["gaps"] = json!(gaps);
            report
        }
        AuthorityCommand::Grant { subject, action, scope, parent } => {
            grant(&mut client, &session, subject, action, scope, parent.as_deref())?
        }
        AuthorityCommand::Revoke { grant: wanted } => revoke(&mut client, &session, wanted)?,
    };
    Ok(report)
}

/// What each ACTIVE instance cannot do, and the command that would let it (D-441).
///
/// Exactly one capability is reported, and the narrowness is the point: `shell@workspace`. The control plane
/// grants it to an instance the **user** creates and not to one an instance spawns (D-61), and that asymmetry is
/// the one whose absence surprises a user — it is why a team's members could not run a command in the round-5
/// measurements (D-256's anatomy: "no member executes anything"). The other asymmetries are the design:
/// `message`/`delegate`/`manage` are the leader's, and `task_result` is issued per task when one is delegated, so
/// listing their absence would be noise about a member being a member. The check uses the runtime's own
/// vocabulary and covering rule (`capability::scope_covers`) rather than a second copy of it, so this view cannot
/// disagree with the dispatch gate about what a grant means.
fn capability_gaps(report: &Json) -> Vec<Json> {
    use teamagents_core::v2::capability::scope_covers;
    let grants: Vec<&Json> = report["grants"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|grant| !grant["revoked"].as_bool().unwrap_or(false))
        .collect();
    let mut gaps = Vec::new();
    for instance in report["instances"].as_array().into_iter().flatten() {
        if instance["lifecycle"].as_str() != Some("ACTIVE") {
            continue;
        }
        let subject = instance["id"].as_str().unwrap_or("");
        let held = grants.iter().any(|grant| {
            grant["subject"].as_str() == Some(subject)
                && grant["action"].as_str() == Some("shell")
                && scope_covers(grant["resource_scope"].as_str().unwrap_or(""), "workspace")
        });
        if held {
            continue;
        }
        gaps.push(json!({
            "instance": subject,
            "missing": "shell@workspace",
            "hand": format!("teamagents authority grant --subject {subject} --action shell --scope workspace"),
        }));
    }
    gaps
}

fn connect(socket: &Path) -> Result<Client, (i32, String)> {
    Client::connect(socket).map_err(|error| {
        (
            2,
            format!(
                "authority: {error}\nauthority works on a running session; start one with `teamagents` \
                 (TUI) or `teamagents exec \"…\"`, or point --state-root at the session you mean"
            ),
        )
    })
}

fn read_grants(client: &mut Client) -> Result<Json, (i32, String)> {
    client.call("grants", json!({})).map_err(|error| (1, format!("authority: cannot read the grants: {error}")))
}

fn grant(
    client: &mut Client,
    session: &Json,
    subject: &str,
    action: &str,
    scope: &str,
    parent: Option<&str>,
) -> Result<Json, (i32, String)> {
    if subject.trim().is_empty() {
        return Err((2, "authority: --subject must name the instance receiving the grant".into()));
    }
    if !capability::ACTIONS.contains(&action) {
        return Err((
            2,
            format!(
                "authority: {action:?} is not an action the runtime checks ({}); \
                 nothing would consult this grant",
                capability::ACTIONS.join(", ")
            ),
        ));
    }
    if !capability::authorizes_something(action, scope) {
        let asked = capability::asks_about(action).map(|shape| shape.example()).unwrap_or("<nothing>");
        return Err((
            2,
            format!(
                "authority: no check asks about {action}@{scope}, so this grant would authorize nothing \
                 (it is never dispatched, never offered, never refused). {action} is asked over {asked}, \
                 and the session scope covers every resource.",
            ),
        ));
    }
    // a subject that does not exist yet is legal (the spawner picks instance
    // ids), so this is reported, not refused
    let note = subject_note(client, subject)?;
    let mut params = json!({"subject": subject, "action": action, "resource_scope": scope});
    if let Some(parent) = parent {
        params["parent_grant_id"] = json!(parent);
    }
    let command_id = format!("authority-grant-{}", uuid::Uuid::new_v4());
    let result = client
        .command(&command_id, "issue_grant", params)
        .map_err(|error| (1, format!("authority: the session refused the grant: {error}")))?;
    let mut report = json!({
        "session_id": session["session_id"], "state_root": session["state_root"],
        "grant_id": result["grant_id"], "revision": result["revision"],
        "subject": subject, "action": action, "resource_scope": scope,
        "parent_grant_id": parent,
    });
    if let Some(note) = note {
        report["note"] = json!(note);
    }
    Ok(report)
}

/// A warning when the subject is not an instance of this session: the grant is
/// written anyway (it takes effect if such an instance ever appears), but a typo
/// is the likelier reading.
fn subject_note(client: &mut Client, subject: &str) -> Result<Option<String>, (i32, String)> {
    let checkpoint = client
        .call("checkpoint", json!({}))
        .map_err(|error| (1, format!("authority: cannot read the instances: {error}")))?;
    let mut known: Vec<String> = checkpoint["snapshot"]["instances"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|instance| instance["id"].as_str().map(str::to_string))
        .collect();
    if known.iter().any(|id| id == subject) {
        return Ok(None);
    }
    known.sort();
    Ok(Some(format!(
        "no instance {subject:?} in this session yet (known: {}); nothing happens until one with that id \
         exists — check the spelling against the TUI instances panel",
        if known.is_empty() { "none".to_string() } else { known.join(", ") }
    )))
}

fn revoke(client: &mut Client, session: &Json, wanted: &str) -> Result<Json, (i32, String)> {
    let view = read_grants(client)?;
    let grants = view["grants"].as_array().cloned().unwrap_or_default();
    let row = resolve(&grants, wanted)?;
    let id = row["id"].as_str().unwrap_or_default().to_string();
    let command_id = format!("authority-revoke-{}", uuid::Uuid::new_v4());
    let result = client
        .command(&command_id, "revoke_grant", json!({"grant_id": id}))
        .map_err(|error| (1, format!("authority: the session refused the revocation: {error}")))?;
    Ok(json!({
        "session_id": session["session_id"], "state_root": session["state_root"],
        "grant_id": id, "grant": row, "revoked": result["revoked"], "revision": result["revision"],
    }))
}

/// The grant a user named, through the shared resolver (D-67 moved it next to the
/// other client surfaces).
fn resolve(grants: &[Json], wanted: &str) -> Result<Json, (i32, String)> {
    super::exec::resolve_prefix(grants, "id", wanted, "grant", "teamagents authority")
}

// ------------------------------------------------------------------ rendering --

fn print_report(options: &AuthorityOptions, report: &Json) {
    if options.json_out {
        println!("{}", serde_json::to_string(&report).unwrap_or_else(|_| "{}".into()));
        return;
    }
    match &options.command {
        AuthorityCommand::List => print_list(report),
        AuthorityCommand::Grant { .. } => {
            println!(
                "granted {} ─{}→ {}  ({}, revision {})",
                report["subject"].as_str().unwrap_or(""),
                report["action"].as_str().unwrap_or(""),
                report["resource_scope"].as_str().unwrap_or(""),
                report["grant_id"].as_str().unwrap_or(""),
                report["revision"].as_i64().unwrap_or(0)
            );
            if let Some(note) = report["note"].as_str() {
                eprintln!("note: {note}");
            }
        }
        AuthorityCommand::Revoke { .. } => {
            let grant = &report["grant"];
            let revoked: Vec<&str> =
                report["revoked"].as_array().into_iter().flatten().filter_map(Json::as_str).collect();
            println!(
                "revoked {} ─{}→ {}  ({}, revision {})",
                grant["subject"].as_str().unwrap_or(""),
                grant["action"].as_str().unwrap_or(""),
                grant["resource_scope"].as_str().unwrap_or(""),
                report["grant_id"].as_str().unwrap_or(""),
                report["revision"].as_i64().unwrap_or(0)
            );
            match revoked.len() {
                0 => println!("nothing was revoked: this grant was already revoked"),
                1 => {}
                n => println!("the cascade also revoked {} derived grants", n - 1),
            }
        }
    }
}

fn print_list(report: &Json) {
    let grants = report["grants"].as_array().cloned().unwrap_or_default();
    println!(
        "session {} ({}), grant revision {}",
        report["session_id"].as_str().unwrap_or(""),
        report["state_root"].as_str().unwrap_or(""),
        report["revision"].as_i64().unwrap_or(0)
    );
    // a grant's subject is an instance id: show them, or the user cannot name one
    let instances = report["instances"].as_array().cloned().unwrap_or_default();
    if instances.is_empty() {
        println!("instances: none yet");
    } else {
        let listed: Vec<String> = instances
            .iter()
            .map(|instance| {
                format!(
                    "{} ({}/{})",
                    instance["id"].as_str().unwrap_or(""),
                    instance["lifecycle"].as_str().unwrap_or(""),
                    instance["phase"].as_str().unwrap_or("")
                )
            })
            .collect();
        println!("instances: {}", listed.join(", "));
    }
    // D-441: the sentence the user was missing — which members cannot do what, before they wonder why nothing
    // happens. Printed whatever the grant list holds, because "no gaps" is information too.
    let gaps = report["gaps"].as_array().cloned().unwrap_or_default();
    if instances.iter().any(|instance| instance["lifecycle"].as_str() == Some("ACTIVE")) {
        if gaps.is_empty() {
            println!("capability gaps: none for the ACTIVE instances");
        } else {
            println!("capability gaps (an instance the user created holds shell@workspace; one an instance spawns does not):");
            for gap in &gaps {
                println!(
                    "  {} cannot {} — issue one with `{}`",
                    gap["instance"].as_str().unwrap_or(""),
                    gap["missing"].as_str().unwrap_or(""),
                    gap["hand"].as_str().unwrap_or("")
                );
            }
        }
    }
    if grants.is_empty() {
        println!("no grants");
        return;
    }
    let cell = |grant: &Json, key: &str| grant[key].as_str().unwrap_or("").to_string();
    let rows: Vec<[String; 5]> = grants
        .iter()
        .map(|grant| {
            [
                cell(grant, "id"),
                cell(grant, "subject"),
                cell(grant, "action"),
                cell(grant, "resource_scope"),
                if grant["revoked"].as_bool().unwrap_or(false) { "revoked".into() } else { "live".into() },
            ]
        })
        .collect();
    let mut widths = [2usize, 7, 6, 5, 5];
    for row in &rows {
        for (index, value) in row.iter().enumerate() {
            widths[index] = widths[index].max(value.chars().count());
        }
    }
    println!(
        "{:<w0$}  {:<w1$}  {:<w2$}  {:<w3$}  STATE",
        "ID",
        "SUBJECT",
        "ACTION",
        "SCOPE",
        w0 = widths[0],
        w1 = widths[1],
        w2 = widths[2],
        w3 = widths[3]
    );
    for row in &rows {
        println!(
            "{:<w0$}  {:<w1$}  {:<w2$}  {:<w3$}  {}",
            row[0],
            row[1],
            row[2],
            row[3],
            row[4],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3]
        );
    }
    let live = rows.iter().filter(|row| row[4] == "live").count();
    println!("{} live, {} revoked", live, rows.len() - live);
    // rows written before the surface existed (or by hand) can name a pair no
    // check consults: say so instead of implying the grant does something
    for grant in &grants {
        let (action, scope) = (cell(grant, "action"), cell(grant, "resource_scope"));
        if !capability::authorizes_something(&action, &scope) {
            println!("note: nothing checks {action}@{scope} ({}), so that grant authorizes nothing", cell(grant, "id"));
        }
    }
}

#[cfg(test)]
mod tests {
    /// D-441: the gaps view names what a member cannot do and the command that would let it. The rule it uses is
    /// the runtime's own (`capability::asks_about`) and its covering (a session-scoped grant covers everything),
    /// so the view cannot disagree with the dispatch gate about what a grant means.
    #[test]
    fn the_authority_view_names_what_an_instance_cannot_do_and_the_hand_that_would_help() {
        let report = json!({
            "instances": [
                {"id": "i-leader", "lifecycle": "ACTIVE", "phase": "READY"},
                {"id": "i-worker", "lifecycle": "ACTIVE", "phase": "READY"},
                {"id": "i-old", "lifecycle": "TERMINATED", "phase": "READY"}
            ],
            "grants": [
                // the user's own instance: shell@workspace by creation (control.rs) plus the collaboration set.
                {"subject": "i-leader", "action": "shell", "resource_scope": "workspace", "revoked": false},
                {"subject": "i-leader", "action": "manage", "resource_scope": "session", "revoked": false},
                {"subject": "i-leader", "action": "delegate", "resource_scope": "session", "revoked": false},
                {"subject": "i-leader", "action": "message", "resource_scope": "session", "revoked": false},
                // a member holds a delegation of one task, which is not a shell capability
                {"subject": "i-worker", "action": "task_result", "resource_scope": "task:t-1", "revoked": false}
            ]
        });
        let gaps = capability_gaps(&report);
        let named: Vec<(String, String)> = gaps
            .iter()
            .map(|gap| {
                (gap["instance"].as_str().unwrap_or("").to_string(), gap["missing"].as_str().unwrap_or("").to_string())
            })
            .collect();
        assert!(
            !named.iter().any(|(instance, _)| instance == "i-leader"),
            "the leader holds everything it is asked about: {named:?}"
        );
        assert!(
            !named.iter().any(|(instance, _)| instance == "i-old"),
            "a terminated instance is not reported: {named:?}"
        );
        // exactly one gap is reported, for the member: the capability whose absence is the surprise. The other
        // asymmetries are the design (a member is not its leader), and the view stays quiet about them.
        assert_eq!(named, vec![("i-worker".to_string(), "shell@workspace".to_string())], "{named:?}");
        // the hand is copy-pasteable, and it is the surface that can actually issue one
        let shell = gaps.iter().find(|gap| gap["missing"] == json!("shell@workspace")).expect("the shell gap");
        assert_eq!(
            shell["hand"],
            json!("teamagents authority grant --subject i-worker --action shell --scope workspace")
        );
        // a session-scoped shell grant covers the workspace shape, which is the covering rule the dispatch gate
        // uses too: move the member's shell grant to "session" and the gap disappears
        let mut session_scoped = report.clone();
        session_scoped["grants"]
            .as_array_mut()
            .unwrap()
            .push(json!({"subject": "i-worker", "action": "shell", "resource_scope": "session", "revoked": false}));
        assert!(
            capability_gaps(&session_scoped).iter().all(|gap| gap["instance"] != json!("i-worker")),
            "a session grant covers it: {:?}",
            capability_gaps(&session_scoped)
        );
    }

    use super::{capability_gaps, resolve, AuthorityCommand};
    use serde_json::json;

    #[test]
    fn a_revocation_takes_the_full_id_or_an_unambiguous_prefix() {
        let grants = vec![
            json!({"id": "g-11111111-2222", "subject": "i-worker"}),
            json!({"id": "g-11112222-3333", "subject": "i-leader"}),
        ];
        assert_eq!(resolve(&grants, "g-11111111-2222").unwrap()["subject"], json!("i-worker"));
        assert_eq!(resolve(&grants, "g-1111").unwrap_err().0, 2, "an ambiguous prefix is refused");
        assert!(resolve(&grants, "g-1111").unwrap_err().1.contains("g-11111111-2222"));
        assert_eq!(resolve(&grants, "g-9999").unwrap_err().0, 2);
        assert!(resolve(&grants, "g-9999").unwrap_err().1.contains("no grant id starts with"));
        let empty = resolve(&grants, "  ").unwrap_err();
        assert_eq!(empty.0, 2);
        assert!(empty.1.contains("needs an id"), "{}", empty.1);
    }

    #[test]
    fn the_commands_are_distinct() {
        assert_ne!(AuthorityCommand::List, AuthorityCommand::Revoke { grant: "g-1".into() });
    }
}
