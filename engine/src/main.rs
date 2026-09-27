//! teamagents: single Rust entry point — CLI, TUI launcher, session daemon and
//! the headless client (`exec`) that shares the daemon with the TUI.

use std::path::{Path, PathBuf};
use teamagents_engine::{cli, tools};

const HELP: &str = "TeamAgents: work with a Leader in your terminal\n\n\
usage: teamagents [--cwd DIR] [--state-root PATH] [--model KEY] [--full-auto]\n\
  teamagents                          TUI attached to your daemon (starts one if needed)\n\
  teamagents exec [--json|--stream-json] [--timeout SEC] [--check CMD] \"…\"   one headless input\n\
  teamagents authority [list] [--json]          the session's grants, with the ids revoke needs\n\
  teamagents authority grant --subject ID --action A --scope S [--parent G]\n\
  teamagents authority revoke --grant ID        revoke that grant and everything derived from it\n\
  teamagents approvals [list] [--json]          the approvals a session is waiting on\n\
  teamagents approvals approve --id ID          approve that call, once (bound to its arguments)\n\
  teamagents approvals deny --id ID             deny it; the operation fails closed\n\
  teamagents instances [list] [--json]          the session's instances\n\
  teamagents instances resume|pause --id ID     let a parked instance run again, or stop one\n\
  teamagents instances terminate --id ID --yes  retire it (workspace and open work handled)\n\
  teamagents tasks [list] [--json]              the session's tasks\n\
  teamagents tasks cancel --id ID               cancel one; a delegator waiting on it is released\n\
  teamagents runners [list] [--json]            the job runners this state root still carries\n\
  teamagents runners stop [--id JOB]            ask them to retire (a runner with a running command refuses)\n\
  teamagents daemon [--state-root PATH] [--cwd DIR] [--model KEY] [--full-auto]\n\
  teamagents daemon --stop [--state-root PATH]   stop that session's daemon (no pid: the socket is the address)\n\
  teamagents init [--state-root PATH]   write config and prepare the state root\n\
  teamagents doctor [--state-root PATH] check config, credentials, state root and host\n\
  teamagents version | --version      print the version\n\
  teamagents --help                   print this help\n\n\
exec reads the prompt from stdin when it is \"-\", runs each --check acceptance command\n\
in the workspace after the turn ends, and exits 0 completed, 1 failed or unfinished,\n\
3 approval required, 124 timeout, 2 usage. --json prints one report object; --stream-json\n\
prints the session's events (one {\"type\":\"event\",…} line each, in order, then the same\n\
report as {\"type\":\"report\",…}) while the run waits, flushed line by line.\n\
authority, approvals, instances and tasks talk to the running session (start it with\n\
teamagents or exec) and exit 0 done, 1 the session refused it, 2 usage. authority is how a\n\
spawned worker gets shell@workspace (§5.1) and how a capability is taken back; approvals\n\
answers the decision that made exec exit 3; instances and tasks are the user-side\n\
interventions of §5.4 (pause/resume/terminate, cancel a task) without starting the TUI.\n\
runners works on a state root rather than a session — it asks each leftover jobs-runner\n\
process to retire, which is what a state root carries after its session is gone; a runner\n\
whose command is still running refuses, so this never takes work away.\n\
The Leader builds the team through spawn/delegate/send/wait; entry points from older\n\
releases (TeamSpec files, line mode, session resume) are not supported.\n\
First run: teamagents init -> set the credential env var -> teamagents doctor -> teamagents.";

fn usage() -> ! {
    eprintln!("{HELP}");
    std::process::exit(2);
}

/// A rejected argument, with the reason first and the help after it. `usage()` alone never said *which*
/// argument was wrong: measured 2026-09-27, `teamagents --nonsense` and `teamagents --timeout abc hi` printed
/// the whole help and never the offending word, so the user had to diff their command against the usage.
fn reject(reason: &str) -> ! {
    eprintln!("{reason}");
    eprintln!();
    usage()
}

/// A flag whose value is missing, empty, or looks like another flag.
fn needs_a_value(flag: &str) -> ! {
    reject(&format!("{flag} needs a value"))
}

/// The same flag twice: the parser keeps one, so saying so beats silently dropping the first.
fn given_twice(flag: &str) -> ! {
    reject(&format!("{flag} was given twice"))
}

/// One `--timeout` rule for its three shapes (missing, not a number, zero): the value bounds a whole turn.
fn bad_timeout() -> ! {
    reject("--timeout needs a whole number of seconds, at least 1")
}

/// A flag or entry point that exists in no release this binary serves: say so
/// and stop, rather than accepting it and doing nothing (D-73, the rule the
/// removed entry points `--plain`/`--resume`/`--team` already follow).
fn refuse(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(2);
}

pub struct Args {
    pub cwd: Option<String>,
    pub resume: Option<String>,
    pub full_auto: bool,
    pub team: Option<String>,
    pub plain: bool,
    pub command: Option<String>,
    pub positional: Option<String>,
    pub state_root: Option<String>,
    pub model: Option<String>,
    pub timeout: Option<u64>,
    pub checks: Vec<String>,
    pub exec_json: bool,
    /// D-249: `exec --stream-json` — the session's events as NDJSON while the run waits, then the report.
    pub stream_json: bool,
    pub subject: Option<String>,
    pub action: Option<String>,
    pub scope: Option<String>,
    pub parent_grant: Option<String>,
    pub grant: Option<String>,
    pub approval_id: Option<String>,
    pub confirmed: bool,
    /// D-248: `daemon --stop` — stop the session's daemon (as opposed to starting one).
    pub daemon_stop: bool,
}

fn parse_args() -> Args {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.len() == 1 {
        match argv[0].as_str() {
            "-h" | "--help" => {
                println!("{HELP}");
                std::process::exit(0);
            }
            "--version" | "-V" => std::process::exit(cli::version()),
            _ => {}
        }
    }
    let mut args = Args {
        cwd: None,
        resume: None,
        full_auto: false,
        team: None,
        plain: false,
        command: None,
        positional: None,
        state_root: None,
        model: None,
        timeout: None,
        checks: Vec::new(),
        exec_json: false,
        subject: None,
        action: None,
        scope: None,
        parent_grant: None,
        grant: None,
        approval_id: None,
        confirmed: false,
        daemon_stop: false,
        stream_json: false,
    };
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--cwd" => {
                if args.cwd.is_some() {
                    given_twice("--cwd");
                }
                let v = argv
                    .get(i + 1)
                    .cloned()
                    .filter(|v| !v.is_empty() && !v.starts_with('-'))
                    .unwrap_or_else(|| needs_a_value("--cwd"));
                args.cwd = Some(v);
                i += 2;
            }
            "--resume" => {
                if args.resume.is_some() {
                    given_twice("--resume");
                }
                let v = argv
                    .get(i + 1)
                    .cloned()
                    .filter(|v| !v.is_empty() && !v.starts_with('-'))
                    .unwrap_or_else(|| needs_a_value("--resume"));
                args.resume = Some(v);
                i += 2;
            }
            "--team" => {
                if args.team.is_some() {
                    given_twice("--team");
                }
                let v = argv
                    .get(i + 1)
                    .cloned()
                    .filter(|v| !v.is_empty() && !v.starts_with('-'))
                    .unwrap_or_else(|| needs_a_value("--team"));
                args.team = Some(v);
                i += 2;
            }
            "--full-auto" => {
                if args.full_auto {
                    given_twice("--full-auto");
                }
                args.full_auto = true;
                i += 1;
            }
            "--plain" => {
                if args.plain {
                    given_twice("--plain");
                }
                args.plain = true;
                i += 1;
            }
            // `-v` was verbose logging in an earlier release and is not a flag
            // this binary honours; a silent accept would leave the user believing
            // they turned something on (D-73). `-V`/`--version` is the version.
            "-v" | "--verbose" => refuse(
                "-v/--verbose is not supported: the daemon writes its log to <state root>/daemon.log \
                 (the path `teamagents daemon` prints). Use --version for the version.",
            ),
            // internal: the controlled shell job runner (§6.2), never user-facing
            "jobs-runner" => {
                if args.command.is_some() {
                    reject("two entry points were given: pick one (teamagents --help lists them)");
                }
                args.command = Some(argv[i].clone());
                args.positional = argv.get(i + 1).cloned();
                i += 2;
            }
            "--state-root" => {
                if args.state_root.is_some() {
                    given_twice("--state-root");
                }
                let v =
                    argv.get(i + 1).cloned().filter(|v| !v.is_empty() && !v.starts_with('-')).unwrap_or_else(|| {
                        reject("--state-root needs a path: an empty value would use the current directory")
                    });
                args.state_root = Some(v);
                i += 2;
            }
            "--model" => {
                if args.model.is_some() {
                    given_twice("--model");
                }
                let v = argv
                    .get(i + 1)
                    .cloned()
                    .filter(|v| !v.is_empty() && !v.starts_with('-'))
                    .unwrap_or_else(|| needs_a_value("--model"));
                args.model = Some(v);
                i += 2;
            }
            "daemon" => {
                if args.command.is_some() {
                    reject("two entry points were given: pick one (teamagents --help lists them)");
                }
                args.command = Some(argv[i].clone());
                i += 1;
            }
            "serve" | "init" | "doctor" | "validate" | "sessions" | "version" | "exec" | "authority" | "approvals"
            | "instances" | "tasks" | "runners" => {
                if args.command.is_some() {
                    reject("two entry points were given: pick one (teamagents --help lists them)");
                }
                // An entry point no release serves is refused *here*, as soon as the word is read, so the
                // message names it instead of whatever flag followed it, and so no flag of a removed
                // subcommand is left in the parser to be accepted and ignored (D-136; D-75's rule, for flags).
                if matches!(argv[i].as_str(), "serve" | "validate" | "sessions") {
                    refuse(&format!(
                        "teamagents {}: this entry point is no longer supported; the Leader builds the team through spawn/delegate and the daemon owns the session.\nRun teamagents for the TUI, or teamagents exec \"…\" for one headless input.",
                        argv[i]
                    ));
                }
                args.command = Some(argv[i].clone());
                if argv[i] == "exec" {
                    args.exec_json = false;
                }
                i += 1;
            }
            "--json"
                if matches!(
                    args.command.as_deref(),
                    Some("exec" | "authority" | "approvals" | "instances" | "tasks" | "runners")
                ) =>
            {
                if args.exec_json {
                    given_twice("--json");
                }
                if args.stream_json {
                    reject("--json and --stream-json are two shapes of the same stdout: pick one");
                }
                args.exec_json = true;
                i += 1;
            }
            // D-249: the streaming shape is an `exec` output mode, not a flag of the read verbs (which print
            // one report and have no event stream of their own to follow).
            "--stream-json" if args.command.as_deref() == Some("exec") => {
                if args.stream_json {
                    given_twice("--stream-json");
                }
                if args.exec_json {
                    reject("--json and --stream-json are two shapes of the same stdout: pick one");
                }
                args.stream_json = true;
                i += 1;
            }
            "--yes" if matches!(args.command.as_deref(), Some("instances" | "tasks")) => {
                if args.confirmed {
                    given_twice("--yes");
                }
                args.confirmed = true;
                i += 1;
            }
            "--id" if matches!(args.command.as_deref(), Some("approvals" | "instances" | "tasks" | "runners")) => {
                if args.approval_id.is_some() {
                    given_twice("--id");
                }
                args.approval_id = Some(
                    argv.get(i + 1)
                        .cloned()
                        .filter(|v| !v.is_empty() && !v.starts_with('-'))
                        .unwrap_or_else(|| needs_a_value("--id")),
                );
                i += 2;
            }
            "--subject" if args.command.as_deref() == Some("authority") => {
                if args.subject.is_some() {
                    given_twice("--subject");
                }
                args.subject = Some(
                    argv.get(i + 1)
                        .cloned()
                        .filter(|v| !v.is_empty() && !v.starts_with('-'))
                        .unwrap_or_else(|| needs_a_value("--subject")),
                );
                i += 2;
            }
            "--action" if args.command.as_deref() == Some("authority") => {
                if args.action.is_some() {
                    given_twice("--action");
                }
                args.action = Some(
                    argv.get(i + 1)
                        .cloned()
                        .filter(|v| !v.is_empty() && !v.starts_with('-'))
                        .unwrap_or_else(|| needs_a_value("--action")),
                );
                i += 2;
            }
            "--scope" if args.command.as_deref() == Some("authority") => {
                if args.scope.is_some() {
                    given_twice("--scope");
                }
                args.scope = Some(
                    argv.get(i + 1)
                        .cloned()
                        .filter(|v| !v.is_empty() && !v.starts_with('-'))
                        .unwrap_or_else(|| needs_a_value("--scope")),
                );
                i += 2;
            }
            "--parent" if args.command.as_deref() == Some("authority") => {
                if args.parent_grant.is_some() {
                    given_twice("--parent");
                }
                args.parent_grant = Some(
                    argv.get(i + 1)
                        .cloned()
                        .filter(|v| !v.is_empty() && !v.starts_with('-'))
                        .unwrap_or_else(|| needs_a_value("--parent")),
                );
                i += 2;
            }
            "--grant" if args.command.as_deref() == Some("authority") => {
                if args.grant.is_some() {
                    given_twice("--grant");
                }
                args.grant = Some(
                    argv.get(i + 1)
                        .cloned()
                        .filter(|v| !v.is_empty() && !v.starts_with('-'))
                        .unwrap_or_else(|| needs_a_value("--grant")),
                );
                i += 2;
            }
            // D-248: `daemon --stop` is the stop lever; `daemon` alone starts one in the foreground.
            "--stop" if args.command.as_deref() == Some("daemon") => {
                args.daemon_stop = true;
                i += 1;
            }
            "--timeout" if args.command.as_deref() == Some("exec") => {
                if args.timeout.is_some() {
                    given_twice("--timeout");
                }
                let raw = argv
                    .get(i + 1)
                    .cloned()
                    .filter(|v| !v.is_empty() && !v.starts_with('-'))
                    .unwrap_or_else(|| bad_timeout());
                let parsed = raw.parse::<u64>().unwrap_or_else(|_| bad_timeout());
                if parsed == 0 {
                    bad_timeout();
                }
                args.timeout = Some(parsed);
                i += 2;
            }
            "--check" if args.command.as_deref() == Some("exec") => {
                args.checks.push(
                    argv.get(i + 1)
                        .cloned()
                        .filter(|v| !v.is_empty() && !v.starts_with('-'))
                        .unwrap_or_else(|| needs_a_value("--check")),
                );
                i += 2;
            }
            other if !other.starts_with('-') || other == "-" => {
                if args.positional.is_some() {
                    reject("this entry point takes one positional argument; quote a multi-word prompt as one");
                }
                args.positional = Some(argv[i].clone());
                i += 1;
            }
            other => reject(&format!(
                "{other}: no entry point this build serves accepts that argument here — `teamagents --help` \
                 shows which flags follow which entry point"
            )),
        }
    }
    if args.command.as_deref() == Some("init")
        && (args.positional.is_some()
            || args.cwd.is_some()
            || args.resume.is_some()
            || args.team.is_some()
            || args.plain
            || args.full_auto)
    {
        reject("init takes no other arguments: it writes the config and prepares the state root");
    }
    args
}

/// Where the TUI binary is, or why it is not there. `Err` carries the message the user sees: an explicit
/// `TEAMAGENTS_TUI` that names nothing is refused with *that* path (D-231) instead of the raw spawn error the
/// first version reported — `cannot start the TUI: No such file or directory (os error 2)`, which named neither
/// the variable nor the path, and was worse than the help a user gets when the variable is unset.
fn find_tui_binary() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("TEAMAGENTS_TUI").filter(|v| !v.is_empty()) {
        let path = PathBuf::from(path);
        if !path.is_file() {
            return Err(format!(
                "TEAMAGENTS_TUI names {}, which is not a file: point it at the teamagents-tui binary, or unset it \
                 so the CLI looks beside itself",
                path.display()
            ));
        }
        return Ok(path);
    }
    let exe = std::env::current_exe().ok();
    if let Some(dir) = exe.as_ref().and_then(|p| p.parent()) {
        let sibling = dir.join("teamagents-tui");
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    // never search the caller's cwd: picking up a binary from an arbitrary
    // project directory is local code execution (P2-7)
    for root in tui_search_roots(exe.as_deref()) {
        for candidate in [
            root.join("tui/target/release/teamagents-tui"),
            root.join("tui/target/debug/teamagents-tui"),
            root.join("target/release/teamagents-tui"),
            root.join("target/debug/teamagents-tui"),
        ] {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    match tools::which("teamagents-tui") {
        Some(path) => Ok(path),
        None => Err("teamagents-tui not found. Install teamagents and teamagents-tui from a release into the same \
                     directory, or point TEAMAGENTS_TUI at the binary.\nFrom source: cargo build --manifest-path \
                     tui/Cargo.toml; headless use: teamagents exec."
            .to_string()),
    }
}

fn tui_search_roots(exe: Option<&std::path::Path>) -> Vec<PathBuf> {
    exe.map(|exe| exe.ancestors().map(Path::to_path_buf).collect()).unwrap_or_default()
}

/// Default entry: one daemon per user owns the session; the TUI is a thin
/// client of its socket (§9). The daemon is started detached when no socket is
/// live, so quitting the TUI never stops the session.
fn run_tui(args: &Args) -> i32 {
    let binary = match find_tui_binary() {
        Ok(binary) => binary,
        Err(message) => {
            eprintln!("{message}");
            return 1;
        }
    };
    let state_root = args.state_root.clone().map(PathBuf::from).unwrap_or_else(teamagents_engine::v2_root);
    let socket = match ensure_daemon(DaemonRequest {
        state_root: &state_root,
        model: args.model.clone(),
        full_auto: args.full_auto,
        cwd: args.cwd.as_deref().map(Path::new),
    }) {
        Ok((socket, started)) => {
            note_session_settings(&socket, args.full_auto, args.cwd.as_deref(), started);
            socket
        }
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    let mut command = std::process::Command::new(&binary);
    command.arg("--daemon").arg(&socket);
    command.arg("--state-root").arg(&state_root);
    // No --cwd here: the workspace belongs to the session the daemon owns, and
    // the client already says so (`note_session_settings`, D-57). Passing a flag
    // the TUI cannot honour was the one thing left of that path (D-73).
    // D-180: no `TEAMAGENTS_ENGINE` either — the TUI never starts the engine, so the value it used to be handed
    // was read by nothing (`find_engine_binary` was dead, and `--engine` is now refused).
    match command.status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(e) => {
            // the path is the one thing the OS error cannot tell the user (D-231)
            eprintln!("cannot start the TUI at {}: {e}", binary.display());
            1
        }
    }
}

/// `teamagents exec`: the same backend as the TUI, one headless input (§9).
fn run_exec(args: &Args) -> i32 {
    let prompt = match resolve_prompt(args.positional.clone(), std::io::stdin().lock()) {
        Ok(prompt) => prompt,
        Err(message) => {
            eprintln!("{message}");
            return 2;
        }
    };
    let state_root = args.state_root.clone().map(PathBuf::from).unwrap_or_else(teamagents_engine::v2_root);
    let socket = match ensure_daemon(DaemonRequest {
        state_root: &state_root,
        model: args.model.clone(),
        full_auto: args.full_auto,
        cwd: args.cwd.as_deref().map(Path::new),
    }) {
        Ok((socket, started)) => {
            note_session_settings(&socket, args.full_auto, args.cwd.as_deref(), started);
            socket
        }
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    // the acceptance checks are the user's own commands and run where the user
    // is working: --cwd when given, otherwise this process's directory
    let workspace = args
        .cwd
        .clone()
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    teamagents_engine::v2::exec::run(teamagents_engine::v2::exec::ExecOptions {
        socket,
        prompt,
        timeout_s: args.timeout.unwrap_or(900),
        json_out: args.exec_json,
        stream_events: args.stream_json,
        checks: args.checks.clone(),
        workspace,
    })
}

/// `teamagents authority`: the user's authority surface (§5.1, D-61). It talks
/// to the running session's socket — it never opens the database, so a grant or
/// a revocation linearizes with driver dispatch like every other command (§9).
fn run_authority(args: &Args) -> i32 {
    use teamagents_engine::v2::authority::{AuthorityCommand, AuthorityOptions};
    let state_root = args.state_root.clone().map(PathBuf::from).unwrap_or_else(teamagents_engine::v2_root);
    let socket = state_root.join("daemon.sock");
    // the verb may be inferred from the flags, so `teamagents authority --grant g-1`
    // and `authority --action shell --subject i-w --scope workspace` both work
    let verb = args.positional.as_deref().unwrap_or_else(|| {
        if args.grant.is_some() {
            "revoke"
        } else if args.subject.is_some() || args.action.is_some() || args.scope.is_some() {
            "grant"
        } else {
            "list"
        }
    });
    let command = match verb {
        "list" => {
            if args.grant.is_some() || args.subject.is_some() || args.action.is_some() || args.scope.is_some() {
                eprintln!(
                    "authority list takes no --grant/--subject/--action/--scope; did you mean `authority grant`?"
                );
                return 2;
            }
            AuthorityCommand::List
        }
        "grant" => {
            let required = [
                ("--subject ID (the instance receiving the grant)", &args.subject),
                ("--action A (one of shell, message, delegate, manage, task_result)", &args.action),
                ("--scope S (session, workspace, instance:<id> or task:<id>)", &args.scope),
            ];
            for (flag, present) in required {
                if present.is_none() {
                    eprintln!(
                        "authority grant needs {flag}\n\
                         usage: teamagents authority grant --subject ID --action A --scope S [--parent G]"
                    );
                    return 2;
                }
            }
            AuthorityCommand::Grant {
                subject: args.subject.clone().unwrap_or_default(),
                action: args.action.clone().unwrap_or_default(),
                scope: args.scope.clone().unwrap_or_default(),
                parent: args.parent_grant.clone(),
            }
        }
        "revoke" => {
            let Some(grant) = args.grant.clone() else {
                eprintln!("authority revoke needs --grant ID (see `teamagents authority` for the ids)");
                return 2;
            };
            AuthorityCommand::Revoke { grant }
        }
        other => {
            eprintln!(
                "authority: unknown command {other:?}; use `teamagents authority [list]`, \
                 `authority grant --subject ID --action A --scope S` or `authority revoke --grant ID`"
            );
            return 2;
        }
    };
    teamagents_engine::v2::authority::run(AuthorityOptions { socket, command, json_out: args.exec_json })
}

/// `teamagents approvals`: the user's approval surface (D-67). A headless session
/// parks an out-of-scope operation on this decision, and `exec` reports it (exit 3);
/// this is how that decision is made without starting the TUI.
fn run_approvals(args: &Args) -> i32 {
    use teamagents_engine::v2::approvals::{ApprovalCommand, ApprovalOptions};
    let state_root = args.state_root.clone().map(PathBuf::from).unwrap_or_else(teamagents_engine::v2_root);
    let verb =
        args.positional.as_deref().unwrap_or_else(|| if args.approval_id.is_some() { "approve" } else { "list" });
    let command = match verb {
        "list" => {
            if args.approval_id.is_some() {
                eprintln!("approvals list takes no --id; use `approvals approve --id ID` or `approvals deny --id ID`");
                return 2;
            }
            ApprovalCommand::List
        }
        "approve" | "deny" => {
            let Some(id) = args.approval_id.clone() else {
                eprintln!("approvals {verb} needs --id ID (see `teamagents approvals` for the ids)");
                return 2;
            };
            if verb == "approve" {
                ApprovalCommand::Approve { id }
            } else {
                ApprovalCommand::Deny { id }
            }
        }
        other => {
            eprintln!(
                "approvals: unknown command {other:?}; use `teamagents approvals [list]`, \
                 `approvals approve --id ID` or `approvals deny --id ID`"
            );
            return 2;
        }
    };
    teamagents_engine::v2::approvals::run(ApprovalOptions {
        socket: state_root.join("daemon.sock"),
        command,
        json_out: args.exec_json,
    })
}

/// `teamagents instances`: the §5.4 instance levers, headless (D-68).
fn run_instances(args: &Args) -> i32 {
    use teamagents_engine::v2::intervene::{InterventionCommand, InterventionOptions};
    let state_root = args.state_root.clone().map(PathBuf::from).unwrap_or_else(teamagents_engine::v2_root);
    let command = match (args.positional.as_deref().unwrap_or("list"), args.approval_id.clone()) {
        ("list", None) => InterventionCommand::ListInstances,
        ("pause", Some(id)) => InterventionCommand::Pause { id },
        ("resume", Some(id)) => InterventionCommand::Resume { id },
        ("terminate", Some(id)) => InterventionCommand::Terminate { id },
        ("list", Some(_)) => {
            eprintln!("instances list takes no --id; use `instances pause|resume|terminate --id ID`");
            return 2;
        }
        (verb @ ("pause" | "resume" | "terminate"), None) => {
            eprintln!("instances {verb} needs --id ID (see `teamagents instances` for the ids)");
            return 2;
        }
        (other, _) => {
            eprintln!(
                "instances: unknown command {other:?}; use `teamagents instances [list]`, \
                 `instances pause|resume|terminate --id ID [--yes]`"
            );
            return 2;
        }
    };
    teamagents_engine::v2::intervene::run(InterventionOptions {
        socket: state_root.join("daemon.sock"),
        command,
        confirmed: args.confirmed,
        json_out: args.exec_json,
    })
}

/// `teamagents tasks`: the session's tasks and the cancel lever (D-68), which is how
/// a delegator waiting on an assignee that stopped is released (D-65).
fn run_tasks(args: &Args) -> i32 {
    use teamagents_engine::v2::intervene::{InterventionCommand, InterventionOptions};
    let state_root = args.state_root.clone().map(PathBuf::from).unwrap_or_else(teamagents_engine::v2_root);
    let command = match (args.positional.as_deref().unwrap_or("list"), args.approval_id.clone()) {
        ("list", None) => InterventionCommand::ListTasks,
        ("cancel", Some(id)) => InterventionCommand::CancelTask { id },
        ("list", Some(_)) => {
            eprintln!("tasks list takes no --id; use `tasks cancel --id ID`");
            return 2;
        }
        ("cancel", None) => {
            eprintln!("tasks cancel needs --id ID (see `teamagents tasks` for the ids)");
            return 2;
        }
        (other, _) => {
            eprintln!("tasks: unknown command {other:?}; use `teamagents tasks [list]` or `tasks cancel --id ID`");
            return 2;
        }
    };
    teamagents_engine::v2::intervene::run(InterventionOptions {
        socket: state_root.join("daemon.sock"),
        command,
        confirmed: args.confirmed,
        json_out: args.exec_json,
    })
}

/// `teamagents runners`: what a state root still carries and how to retire it (D-250).
///
/// Deliberately not a client of the running session: the leftover this exists for is the one whose session is
/// gone. It talks to the *runners* (whose token socket is in each job directory's `job.json`), so a session
/// that is live is only reported, never required.
fn run_runners(args: &Args) -> i32 {
    use teamagents_engine::v2::runners::{RunnersCommand, RunnersOptions};
    let state_root = args.state_root.clone().map(PathBuf::from).unwrap_or_else(teamagents_engine::v2_root);
    let command = match (args.positional.as_deref().unwrap_or("list"), args.approval_id.clone()) {
        ("list", None) => RunnersCommand::List,
        ("stop", job) => RunnersCommand::Stop { job_id: job },
        ("list", Some(_)) => {
            eprintln!("runners list takes no --id; use `runners stop --id JOB`");
            return 2;
        }
        (other, _) => {
            eprintln!(
                "runners: unknown command {other:?}; use `teamagents runners [list]` or `runners stop [--id JOB]`"
            );
            return 2;
        }
    };
    teamagents_engine::v2::runners::run(RunnersOptions { state_root, command, json_out: args.exec_json })
}

/// Say it out loud when `--full-auto` could not apply: the mode belongs to the
/// session, which was started earlier (D-41). Silence here was how a documented
/// flag became a no-op that nobody noticed.
fn note_session_settings(socket: &Path, full_auto: bool, cwd: Option<&str>, started: bool) {
    if started || (!full_auto && cwd.is_none()) {
        return;
    }
    let greeting = std::os::unix::net::UnixStream::connect(socket)
        .ok()
        .and_then(|stream| {
            use std::io::{BufRead, BufReader};
            let mut line = String::new();
            BufReader::new(stream).read_line(&mut line).ok()?;
            serde_json::from_str::<serde_json::Value>(&line).ok()
        })
        .unwrap_or(serde_json::Value::Null);
    let described = |key: &str| greeting[key].as_str().unwrap_or("unknown").to_string();
    if full_auto {
        eprintln!(
            "note: a session is already running for this state root in {} mode, so --full-auto did not apply. \
             Stop that daemon first (SIGTERM its pid — `ps -eo pid,args | grep \"[t]eamagents daemon\"` names \
             the pid and state root; a detached daemon has no terminal to Ctrl-C in), or use another \
             --state-root to start in full_auto.",
            described("permissions")
        );
    }
    if let Some(requested) = cwd {
        let live = described("workspace");
        let same = std::fs::canonicalize(requested).ok() == std::fs::canonicalize(&live).ok();
        if !same {
            eprintln!(
                "note: that session works in {live}, so --cwd {requested} did not apply. \
                 Stop that daemon or use another --state-root to work in {requested}."
            );
        }
    }
}

/// The prompt for `exec`: the positional argument, or everything piped into
/// stdin when that argument is `-` (so scripts can feed a long instruction).
fn resolve_prompt(positional: Option<String>, mut stdin: impl std::io::Read) -> Result<String, String> {
    let Some(prompt) = positional else {
        return Err("exec needs a prompt: teamagents exec [--json] [--timeout SEC] [--check CMD] \"…\" (or - to read it from stdin)".into());
    };
    let text = if prompt == "-" {
        let mut buffer = String::new();
        stdin.read_to_string(&mut buffer).map_err(|e| format!("exec: cannot read stdin: {e}"))?;
        buffer.trim_end_matches(['\r', '\n']).to_string()
    } else {
        prompt
    };
    if text.trim().is_empty() {
        return Err("exec: the prompt is empty".into());
    }
    Ok(text)
}

/// What a client asks the daemon it may start to boot with. Workspace and
/// permission mode are session settings: they are fixed when the daemon starts,
/// so a client that joins a running session can only report what it finds
/// (D-41/D-55/D-57). Returns whether a daemon had to be started, so the caller
/// can tell the difference between "started with your settings" and "joined a
/// session that already has its own".
struct DaemonRequest<'a> {
    state_root: &'a Path,
    model: Option<String>,
    full_auto: bool,
    cwd: Option<&'a Path>,
}

fn ensure_daemon(request: DaemonRequest<'_>) -> Result<(PathBuf, bool), String> {
    let DaemonRequest { state_root, model, full_auto, cwd } = request;
    // D-163, before the liveness probe: a `--cwd` that is not a directory can never be honoured — not by a
    // session this client starts, and not by one that is already running (which keeps its own workspace) —
    // and the same path is what `--check` would run the user's acceptance commands in. So it is refused
    // here, in the client's own words, rather than relayed later as a daemon start failure or an opaque
    // check error. `cli::daemon` applies the same rule to a session started by hand.
    if let Some(cwd) = cwd {
        cli::require_workspace_dir(cwd)?;
    }
    // D-166: the same for the root itself — a state root that is a file cannot hold the socket, so the client
    // says so instead of spawning a daemon that dies with `File exists (os error 17)`.
    cli::require_state_root_dir(state_root)?;
    let socket = state_root.join("daemon.sock");
    // D-227: and for a root whose socket path crosses Linux's `sun_path` limit — the daemon refuses it too, but
    // the client should not spawn one to read that back (the same reasoning as D-163/D-166 above).
    cli::require_socket_path_fits(&socket)?;
    // D-228: a directory named `daemon.sock` (or `session.sqlite`) used to answer `Connection refused … start
    // teamagents daemon first`, which points at the wrong thing.
    cli::require_state_paths_kind(state_root)?;
    // liveness is a *connection*, not the presence of a socket file: a crashed
    // daemon leaves a stale file that would make bind fail if we kept it
    if socket.exists() {
        if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            return Ok((socket, false));
        }
        let _ = std::fs::remove_file(&socket);
    }
    let exe = std::env::current_exe().map_err(|e| format!("current exe: {e}"))?;
    let model = model.or_else(|| default_model_key(cwd));
    let mut command = if which_binary("setsid").is_some() {
        let mut command = std::process::Command::new("setsid");
        command.arg(&exe);
        command
    } else {
        std::process::Command::new(&exe)
    };
    command.arg("daemon").arg("--state-root").arg(state_root);
    if let Some(model) = model {
        command.arg("--model").arg(model);
    }
    if full_auto {
        command.arg("--full-auto");
    }
    // without this the daemon (and every instance it drives) works in whatever
    // directory this client was started from, silently ignoring --cwd
    if let Some(cwd) = cwd {
        command.arg("--cwd").arg(cwd);
    }
    // A detached daemon has no terminal to complain on: its banner, its startup
    // failure and anything it logs later land in <state root>/daemon.log.
    std::fs::create_dir_all(state_root).map_err(|e| cli::state_root_uncreatable(state_root, &e))?;
    let log_path = state_root.join("daemon.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| cli::daemon_log_unopenable(&log_path, &e))?;
    let log_start = log.metadata().map(|meta| meta.len()).unwrap_or(0);
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(log);
    let mut child = command.spawn().map_err(|e| format!("cannot start the daemon: {e}"))?;
    for _ in 0..150 {
        if socket.exists() {
            return Ok((socket, true));
        }
        // The daemon reports a startup failure by exiting non-zero: report that
        // now, with its own words, instead of waiting out the whole window.
        // (setsid may fork, so a clean exit here is not a failure signal.)
        match child.try_wait() {
            Ok(Some(status)) if !status.success() => {
                let detail = tail_from(&log_path, log_start, 600);
                return Err(format!(
                    "the daemon exited while starting ({status}): {detail}\nsee {} or run teamagents daemon --state-root {} by hand",
                    log_path.display(),
                    state_root.display()
                ));
            }
            Ok(_) => {}
            Err(error) => return Err(format!("cannot check the daemon process: {error}")),
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Err(format!(
        "the daemon is not ready at {} ({}); run teamagents daemon by hand to see why",
        socket.display(),
        tail_from(&log_path, log_start, 600)
    ))
}

/// The daemon's own output since `from`, for a startup error message: capped so
/// a long log never floods the caller's terminal.
fn tail_from(path: &std::path::Path, from: u64, cap: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::from("the daemon wrote nothing");
    };
    if file.seek(SeekFrom::Start(from)).is_err() {
        return String::from("the daemon wrote nothing");
    }
    let mut text = String::new();
    if file.take(cap as u64).read_to_string(&mut text).is_err() || text.trim().is_empty() {
        return String::from("the daemon wrote nothing");
    }
    text.trim().to_string()
}

/// The leader's model key when the caller did not choose one: the documented default, else the only configured
/// key — read through the same loader the daemon uses (D-244), so the client and the session cannot disagree
/// about the catalog a repository-local config contributes.
fn default_model_key(cwd: Option<&std::path::Path>) -> Option<String> {
    let cwd = match cwd {
        Some(path) => std::path::PathBuf::from(path),
        None => std::env::current_dir().ok()?,
    };
    let catalog = teamagents_engine::config::load_user_config_for(&cwd).ok()?.0;
    if catalog.models.contains_key("leader_main") {
        return Some("leader_main".to_string());
    }
    let mut keys: Vec<String> = catalog.models.keys().cloned().collect();
    keys.sort();
    match keys.len() {
        1 => keys.pop(),
        _ => None,
    }
}

fn which_binary(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .and_then(|paths| std::env::split_paths(&paths).map(|dir| dir.join(name)).find(|candidate| candidate.is_file()))
}

fn main() {
    let args = parse_args();
    let code = match args.command.as_deref() {
        Some("jobs-runner") => match &args.positional {
            Some(dir) => {
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
                match runtime {
                    Ok(runtime) => match runtime.block_on(teamagents_engine::jobs::runner::serve(Path::new(dir))) {
                        Ok(()) => 0,
                        Err(e) => {
                            eprintln!("jobs-runner: {e}");
                            1
                        }
                    },
                    Err(e) => {
                        eprintln!("jobs-runner runtime: {e}");
                        1
                    }
                }
            }
            None => usage(),
        },
        Some("daemon") if args.daemon_stop => {
            // D-73's rule for this verb: it refuses what it does not honour, so a flag of the *starting* shape
            // is named rather than ignored (a stop has no workspace, model or permission mode to choose).
            for (flag, given) in [("--cwd", &args.cwd), ("--model", &args.model)] {
                if given.is_some() {
                    reject(&format!("daemon --stop does not take {flag}: it stops the running session as it is"));
                }
            }
            if args.full_auto {
                reject("daemon --stop does not take --full-auto: it stops the running session as it is");
            }
            cli::daemon_stop(args.state_root.clone().map(PathBuf::from))
        }
        Some("daemon") => cli::daemon(args.state_root.clone(), args.cwd.clone(), args.model.clone(), args.full_auto),
        Some("init") => cli::init(args.state_root.clone().map(PathBuf::from)),
        Some("doctor") => cli::doctor(args.state_root.clone().map(PathBuf::from)),
        Some("version") => cli::version(),
        Some("exec") => run_exec(&args),
        Some("authority") => run_authority(&args),
        Some("approvals") => run_approvals(&args),
        Some("instances") => run_instances(&args),
        Some("tasks") => run_tasks(&args),
        Some("runners") => run_runners(&args),
        _ if args.plain || args.resume.is_some() || args.team.is_some() => {
            eprintln!("--plain/--resume/--team are no longer supported; use teamagents (TUI) or teamagents exec.");
            2
        }
        // A bare word is not a TUI option: `teamagents` opens the TUI and
        // `teamagents exec "…"` runs one headless input. Starting a session and
        // dropping the word would lose exactly what the user typed — and a typo
        // (`teamagents exex "…"`) with it (D-73).
        _ if args.positional.is_some() => {
            eprintln!(
                "teamagents: {:?} is not an entry point, and the TUI takes no prompt.\n\
                 Run `teamagents` for the TUI, or `teamagents exec \"…\"` for one headless input.\n\
                 `teamagents --help` lists every entry point.",
                args.positional.as_deref().unwrap_or("")
            );
            2
        }
        _ => run_tui(&args),
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::resolve_prompt;

    #[test]
    fn exec_takes_the_prompt_from_the_argument_or_from_stdin() {
        assert_eq!(resolve_prompt(Some("do it".into()), &b""[..]).unwrap(), "do it");
        // "-" is the documented stdin marker: the trailing newline an echo adds
        // must not become part of the instruction
        assert_eq!(resolve_prompt(Some("-".into()), &b"line one\nline two\n"[..]).unwrap(), "line one\nline two");
        // a piped prompt may be several lines
        assert_eq!(resolve_prompt(Some("-".into()), &b"a\r\nb\r\n"[..]).unwrap(), "a\r\nb");
    }

    #[test]
    fn exec_refuses_a_missing_or_empty_prompt() {
        let error = resolve_prompt(None, &b""[..]).expect_err("a missing prompt is a usage error");
        assert!(error.contains("stdin"), "the message points at the stdin marker: {error}");
        assert!(resolve_prompt(Some("-".into()), &b"\n\n"[..]).is_err(), "empty stdin has nothing to send");
        assert!(resolve_prompt(Some("   ".into()), &b""[..]).is_err(), "a blank argument has nothing to send");
    }

    #[test]
    fn tui_search_roots_exclude_cwd() {
        // P2-7: executing a binary found under the caller's cwd would be local
        // code execution from an arbitrary repository
        // the contract: roots come from the *binary's* location, never from the
        // caller's cwd (an exe installed in-tree has the repo as an ancestor,
        // which is legitimate — the danger was searching an arbitrary cwd)
        let cwd = std::env::current_dir().unwrap();
        let exe_elsewhere = std::path::Path::new("/usr/local/bin/teamagents");
        let roots = super::tui_search_roots(Some(exe_elsewhere));
        assert_eq!(
            roots,
            vec![
                std::path::PathBuf::from("/usr/local/bin/teamagents"),
                std::path::PathBuf::from("/usr/local/bin"),
                std::path::PathBuf::from("/usr/local"),
                std::path::PathBuf::from("/usr"),
                std::path::PathBuf::from("/"),
            ]
        );
        assert!(!roots.contains(&cwd), "cwd must not be a search root for teamagents-tui");
    }
}
