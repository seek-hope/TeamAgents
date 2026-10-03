//! teamagents: single Rust entry point — CLI, TUI launcher, session daemon and
//! the headless client (`exec`) that shares the daemon with the TUI.

use std::path::{Path, PathBuf};
use teamagents_engine::{cli, tools};

const HELP: &str = "TeamAgents: work with a Leader in your terminal\n\n\
usage: teamagents [--cwd DIR] [--state-root PATH] [--session ID] [--model KEY] [--full-auto]\n\
  teamagents                          TUI attached to your daemon (starts one if needed)\n\
  teamagents exec [--json|--stream-json] [--timeout SEC] [--check CMD] [--accept ID=CMD] [--] \"…\"   one headless input\n\
  teamagents authority [list] [--json]          the session's grants, with the ids revoke needs\n\
  teamagents authority grant --subject ID --action A --scope S [--parent G]\n\
  teamagents authority revoke --grant ID        revoke that grant and everything derived from it\n\
  teamagents approvals [list] [--json]          the approvals a session is waiting on\n\
  teamagents goals [list] [--json]              the goals the session carries (D-267)\n\
  teamagents goals open --id ID [--attach X]    open the next goal, optionally attached to an instance,\n\
      [--check ID=COMMAND]… [--deadline MIN]    with the user's own required checks and a deadline\n\
  teamagents goals cancel --id ID               close that goal and release the instance its refusal parked\n\
  teamagents approvals approve --id ID          approve that call, once (bound to its arguments)\n\
  teamagents approvals deny --id ID             deny it; the operation fails closed\n\
  teamagents instances [list] [--json]          the session's instances\n\
  teamagents instances resume|pause --id ID     let a parked instance run again, or stop one\n\
  teamagents instances interrupt --id ID        cancel the running turn so a queued instruction takes over\n\
  teamagents instances terminate --id ID --yes  retire it (workspace and open work handled)\n\
  teamagents instances merge --id ID            bring a git_worktree member's branch into the session tree\n\
  teamagents sessions [list] [--json]           the sessions this state root carries (D-364)\n\
  teamagents sessions new [--name NAME]         create a named session beside the default one\n\
  teamagents sessions fork --id ID [--name NAME] branch a session's conversation into a new one (D-365)\n\
  teamagents sessions rename --id ID --name NAME  give a named session a new display name\n\
  teamagents sessions restore --id ID           bring an archived session back\n\
  teamagents sessions search --query TEXT [--limit N]  search every session's conversation (read-only, D-368)\n\
  teamagents sessions archive --id ID           move a named session aside (refused while a daemon runs it)\n\
  teamagents sessions delete --id ID --yes      remove a named session's directory and history\n\
  teamagents automations [list] [--json]        schedules that start a goal on their own (D-367)\n\
  teamagents automations add --every MIN --prompt TEXT [--name NAME]   create one\n\
  teamagents automations pause|resume --id ID   stop it, or re-arm it one period out\n\
  teamagents automations remove --id ID --yes   delete it\n\
  teamagents tasks [list] [--json]              the session's tasks\n\
  teamagents tasks cancel --id ID               cancel one; a delegator waiting on it is released\n\
  teamagents artifacts [list] [--json]          what this state root holds on disk (bytes, owner, presence)\n\
  teamagents mcp list                           the tools each configured MCP service offers (D-399)\n\
  teamagents artifacts gc [--json]              collect the artifacts nothing references (needs no session)\n\
  teamagents runners [list] [--json]            the job runners this state root still carries\n\
  teamagents surface [--id INSTANCE] [--json]   what each model request was offered: tool names, authorized\n\
  teamagents runners stop [--id JOB]            ask them to retire (a runner with a running command refuses)\n\
  teamagents runners stop --service --yes [--id JOB]   stop the group a settled command left behind\n\
  teamagents daemon [--state-root PATH] [--cwd DIR] [--model KEY] [--full-auto]\n\
  teamagents daemon --stop [--state-root PATH]   stop that session's daemon (no pid: the socket is the address)\n\
  teamagents init [--state-root PATH]   write config and prepare the state root\n\
  teamagents doctor [--state-root PATH] check config, credentials, state root and host\n\
  teamagents version | --version      print the version\n\
  teamagents --help                   print this help\n\n\
exec reads the prompt from stdin when it is \"-\", runs each --check acceptance command\n\
in the workspace after the turn ends (a verdict on the exit code), and attaches each\n\
--accept ID=COMMAND to the goal before the input lands (a gate the runtime repairs\n\
against at the completion boundary), and exits 0 completed, 1 failed or unfinished,\n\
3 approval required, 124 timeout, 2 usage. --json prints one report object; --stream-json\n\
prints the session's events (one {\"type\":\"event\",…} line each, in order, then the same\n\
report as {\"type\":\"report\",…}) while the run waits, flushed line by line.\n\
authority, approvals, instances and tasks talk to the running session (start it with\n\
teamagents or exec) and exit 0 done, 1 the session refused it, 2 usage. authority is how a\n\
spawned worker gets shell@workspace (§5.1) and how a capability is taken back; approvals\n\
answers the decision that made exec exit 3; instances and tasks are the user-side\n\
interventions of §5.4 (pause/resume/terminate, cancel a task) without starting the TUI.\n\
sessions manages the named sessions under one state root; `--session ID` attaches the TUI, exec or daemon to\n\none of them (the default session is the state root itself). A named session is its own state root, so several\n\nrun side by side, and archive/delete refuse a session that still has a daemon.\n\
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
    /// D-364: `--session ID` — attach to a named session recorded in the state root's `sessions.json`.
    pub session: Option<String>,
    pub model: Option<String>,
    pub timeout: Option<u64>,
    pub checks: Vec<String>,
    /// D-385: `exec --accept ID=COMMAND` — goal-level required checks attached to the goal before the input lands.
    pub accept: Vec<String>,
    /// D-390: `--` was seen — everything after it is a positional argument, never a flag.
    pub after_double_dash: bool,
    pub exec_json: bool,
    /// D-249: `exec --stream-json` — the session's events as NDJSON while the run waits, then the report.
    pub stream_json: bool,
    /// D-251: `runners stop --service` — stop the process group a settled command left behind.
    pub service_stop: bool,
    pub subject: Option<String>,
    pub action: Option<String>,
    pub scope: Option<String>,
    pub parent_grant: Option<String>,
    pub grant: Option<String>,
    pub approval_id: Option<String>,
    pub confirmed: bool,
    /// D-248: `daemon --stop` — stop the session's daemon (as opposed to starting one).
    pub daemon_stop: bool,
    /// D-267: `goals open --attach INSTANCE` — attach the new goal to that instance, which is what makes a later
    /// delegation charge to it (D-266).
    pub attach: Option<String>,
    /// D-364: `sessions new --name NAME`.
    pub session_name: Option<String>,
    /// D-367: `automations add --every MINUTES --prompt TEXT`.
    pub every_minutes: Option<u64>,
    pub prompt: Option<String>,
    /// D-368: `sessions search --query TEXT [--limit N]`.
    pub query: Option<String>,
    pub limit: Option<u64>,
    /// D-267: `goals open --deadline MINUTES` — a deadline for the new goal, measured from now (§8).
    pub deadline_minutes: Option<u64>,
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
        session: None,
        model: None,
        timeout: None,
        checks: Vec::new(),
        accept: Vec::new(),
        after_double_dash: false,
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
        service_stop: false,
        attach: None,
        session_name: None,
        every_minutes: None,
        prompt: None,
        query: None,
        limit: None,
        deadline_minutes: None,
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
            "--session" => {
                if args.session.is_some() {
                    given_twice("--session");
                }
                let v = argv
                    .get(i + 1)
                    .cloned()
                    .filter(|v| !v.is_empty() && !v.starts_with('-'))
                    .unwrap_or_else(|| needs_a_value("--session"));
                args.session = Some(v);
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
            "serve" | "init" | "doctor" | "validate" | "sessions" | "automations" | "version" | "exec"
            | "authority" | "approvals" | "instances" | "tasks" | "runners" | "artifacts" | "goals" | "surface"
            | "mcp" => {
                if args.command.is_some() {
                    reject("two entry points were given: pick one (teamagents --help lists them)");
                }
                // An entry point no release serves is refused *here*, as soon as the word is read, so the
                // message names it instead of whatever flag followed it, and so no flag of a removed
                // subcommand is left in the parser to be accepted and ignored (D-136; D-75's rule, for flags).
                if matches!(argv[i].as_str(), "serve" | "validate") {
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
                    Some(
                        "exec"
                            | "authority"
                            | "approvals"
                            | "instances"
                            | "tasks"
                            | "runners"
                            | "artifacts"
                            | "goals"
                            | "surface"
                            | "sessions"
                            | "automations"
                    )
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
            "--yes"
                if matches!(
                    args.command.as_deref(),
                    Some("instances" | "tasks" | "runners" | "sessions" | "automations")
                ) =>
            {
                if args.confirmed {
                    given_twice("--yes");
                }
                args.confirmed = true;
                i += 1;
            }
            // D-251: the other leftover — the process group a settled command left behind (a service started
            // with `&`). It signals processes rather than asking a runner, so it is its own flag and needs
            // `--yes`.
            "--service" if args.command.as_deref() == Some("runners") => {
                if args.service_stop {
                    given_twice("--service");
                }
                args.service_stop = true;
                i += 1;
            }
            "--id"
                if matches!(
                    args.command.as_deref(),
                    Some(
                        "approvals"
                            | "instances"
                            | "tasks"
                            | "runners"
                            | "goals"
                            | "surface"
                            | "sessions"
                            | "automations"
                    )
                ) =>
            {
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
            "--name" if matches!(args.command.as_deref(), Some("sessions" | "automations")) => {
                if args.session_name.is_some() {
                    given_twice("--name");
                }
                args.session_name = Some(
                    argv.get(i + 1)
                        .cloned()
                        .filter(|v| !v.is_empty() && !v.starts_with('-'))
                        .unwrap_or_else(|| needs_a_value("--name")),
                );
                i += 2;
            }
            "--every" if args.command.as_deref() == Some("automations") => {
                if args.every_minutes.is_some() {
                    given_twice("--every");
                }
                args.every_minutes = Some(
                    argv.get(i + 1).and_then(|v| v.parse::<u64>().ok()).unwrap_or_else(|| needs_a_value("--every")),
                );
                i += 2;
            }
            "--prompt" if args.command.as_deref() == Some("automations") => {
                if args.prompt.is_some() {
                    given_twice("--prompt");
                }
                args.prompt = Some(
                    argv.get(i + 1).cloned().filter(|v| !v.is_empty()).unwrap_or_else(|| needs_a_value("--prompt")),
                );
                i += 2;
            }
            "--query" if args.command.as_deref() == Some("sessions") => {
                if args.query.is_some() {
                    given_twice("--query");
                }
                args.query = Some(
                    argv.get(i + 1).cloned().filter(|v| !v.is_empty()).unwrap_or_else(|| needs_a_value("--query")),
                );
                i += 2;
            }
            "--limit" if args.command.as_deref() == Some("sessions") => {
                if args.limit.is_some() {
                    given_twice("--limit");
                }
                args.limit = Some(
                    argv.get(i + 1).and_then(|v| v.parse::<u64>().ok()).unwrap_or_else(|| needs_a_value("--limit")),
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
            "--attach" if args.command.as_deref() == Some("goals") => {
                if args.attach.is_some() {
                    given_twice("--attach");
                }
                args.attach = Some(
                    argv.get(i + 1)
                        .cloned()
                        .filter(|v| !v.is_empty() && !v.starts_with('-'))
                        .unwrap_or_else(|| needs_a_value("--attach")),
                );
                i += 2;
            }
            "--deadline" if args.command.as_deref() == Some("goals") => {
                if args.deadline_minutes.is_some() {
                    given_twice("--deadline");
                }
                args.deadline_minutes = Some(
                    argv.get(i + 1).and_then(|v| v.parse::<u64>().ok()).unwrap_or_else(|| needs_a_value("--deadline")),
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
            "--check" if matches!(args.command.as_deref(), Some("exec" | "goals")) => {
                args.checks.push(
                    argv.get(i + 1)
                        .cloned()
                        .filter(|v| !v.is_empty() && !v.starts_with('-'))
                        .unwrap_or_else(|| needs_a_value("--check")),
                );
                i += 2;
            }
            "--accept" if matches!(args.command.as_deref(), Some("exec")) => {
                args.accept.push(
                    argv.get(i + 1)
                        .cloned()
                        .filter(|v| !v.is_empty() && !v.starts_with('-'))
                        .unwrap_or_else(|| needs_a_value("--accept")),
                );
                i += 2;
            }
            // D-390: `--` ends option parsing, so a prompt that *starts* with a dash (a bullet list, a diff, a
            // negative number) is an argument rather than an unknown flag. Without it the only way to submit
            // such a prompt was stdin, and `teamagents exec "- a bullet"` was refused with the usage text —
            // measured on a benchmark task whose instruction begins with "- ", where every attempt died at the
            // parser (exit 2) before the model was reached.
            "--" => {
                args.after_double_dash = true;
                i += 1;
            }
            other if args.after_double_dash || !other.starts_with('-') || other == "-" => {
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
        accept: match parse_accept(&args.accept) {
            Ok(checks) => checks,
            Err(code) => return code,
        },
        accept_goal: None,
        workspace,
    })
}

/// `exec --accept ID=COMMAND` → the checks `require_checks` receives (D-385). A spec without `=` is a usage
/// error, not a guess: an id that is not the user's is not an acceptance check the runtime can report back.
fn parse_accept(specs: &[String]) -> Result<Vec<serde_json::Value>, i32> {
    let mut checks = Vec::new();
    for spec in specs {
        let Some((id, command)) = spec.split_once('=') else {
            eprintln!("exec --accept wants ID=COMMAND, got {spec:?}");
            return Err(2);
        };
        if id.is_empty() || command.trim().is_empty() {
            eprintln!("exec --accept wants a non-empty id and command, got {spec:?}");
            return Err(2);
        }
        checks.push(serde_json::json!({"id": id, "command": command}));
    }
    Ok(checks)
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
/// `teamagents mcp list`: what the configured MCP services actually offer (D-399).
///
/// The comparison's Pi column has `pi mcp list`; here the surface was the config file plus `doctor`, and since
/// D-376 the tools are reachable only through codemode — so a user had no way to see them without asking a model.
/// This connects to each configured service and prints its tools, exiting 1 when any service failed (so a CI job
/// can catch a broken one) while still listing the healthy ones.
fn run_mcp(args: &Args) -> i32 {
    if args.positional.as_deref().is_some_and(|verb| verb != "list") {
        eprintln!("mcp takes no sub-verb other than `list`: teamagents mcp list [--cwd DIR]");
        return 2;
    }
    let workspace = args
        .cwd
        .clone()
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let (catalog, _project) = match teamagents_engine::config::load_user_config_for(&workspace) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("mcp: {error}");
            return 2;
        }
    };
    let services = teamagents_engine::bound::list_services(&catalog, &workspace);
    if services.is_empty() {
        println!("no MCP services are configured: bind one in the user config as [tools.<name>] with kind = \"mcp\"");
        return 0;
    }
    let mut failed = 0;
    for (label, listed) in services {
        match listed {
            Ok(tools) => {
                println!("{label}: {} tool(s)", tools.len());
                for (name, description) in tools {
                    println!("  {name} — {description}");
                }
            }
            Err(error) => {
                failed += 1;
                println!("{label}: unavailable — {error}");
            }
        }
    }
    if failed > 0 {
        eprintln!("{failed} MCP service(s) could not be listed");
        1
    } else {
        0
    }
}

/// `teamagents goals`: the user's goal surface (D-267) — which goals the session carries, and how to open the
/// next one. A settled goal cannot be reopened, so this is the lever a session longer than one goal needs.
fn run_goals(args: &Args) -> i32 {
    use teamagents_engine::v2::goals::{GoalCommand, GoalOptions};
    let state_root = args.state_root.clone().map(PathBuf::from).unwrap_or_else(teamagents_engine::v2_root);
    let verb = args.positional.as_deref().unwrap_or("list");
    let command = match verb {
        "list" => {
            if args.approval_id.is_some() || args.attach.is_some() {
                eprintln!("goals list takes no --id/--attach; use `goals open --id ID [--attach INSTANCE]`");
                return 2;
            }
            GoalCommand::List
        }
        "open" => {
            let Some(id) = args.approval_id.clone() else {
                eprintln!("goals open needs --id ID (any name you will recognize later)");
                return 2;
            };
            // --check ID=COMMAND, repeatable: the user's own required checks on the new goal (§8)
            let mut required_checks = Vec::new();
            for spec in &args.checks {
                let Some((check_id, command)) = spec.split_once('=') else {
                    eprintln!("goals open --check wants ID=COMMAND, got {spec:?}");
                    return 2;
                };
                if check_id.is_empty() || command.trim().is_empty() {
                    eprintln!("goals open --check wants a non-empty id and command, got {spec:?}");
                    return 2;
                }
                required_checks.push(serde_json::json!({"id": check_id, "command": command}));
            }
            let deadline = args.deadline_minutes.map(|minutes| teamagents_core::models::now() + (minutes * 60) as f64);
            GoalCommand::Open { id, attach: args.attach.clone(), required_checks, deadline }
        }
        "cancel" => {
            let Some(id) = args.approval_id.clone() else {
                eprintln!("goals cancel needs --id ID (the goal to close)");
                return 2;
            };
            if args.attach.is_some() || !args.checks.is_empty() || args.deadline_minutes.is_some() {
                eprintln!("goals cancel takes only --id: it closes the goal and releases what its refusal parked");
                return 2;
            }
            GoalCommand::Cancel { id }
        }
        other => {
            eprintln!(
                "goals: unknown command {other:?}; use `teamagents goals [list]`, \
                 `goals open --id ID [--attach INSTANCE] [--check ID=COMMAND]… [--deadline MINUTES]`, or \
                 `goals cancel --id ID`"
            );
            return 2;
        }
    };
    teamagents_engine::v2::goals::run(GoalOptions {
        socket: state_root.join("daemon.sock"),
        command,
        json_out: args.exec_json,
    })
}

/// Resolve `--session ID` to the named session's directory, so every entry point that already takes
/// `--state-root` works unchanged (D-364). The state root given on the command line is the *base* here: it is
/// the directory holding `sessions.json`. `sessions` commands take `--id`, not `--session`.
fn resolve_session(args: &mut Args) -> Result<(), String> {
    let Some(id) = args.session.clone() else { return Ok(()) };
    if args.command.as_deref() == Some("sessions") {
        return Err("--session attaches to a session; `sessions` commands take --id".into());
    }
    let home = args.state_root.clone().map(PathBuf::from).unwrap_or_else(teamagents_engine::v2_root);
    let registry = teamagents_engine::v2::sessions::Registry::load(&home)?;
    let dir = registry.resolve(&id)?;
    args.state_root = Some(dir.to_string_lossy().into_owned());
    Ok(())
}

/// `teamagents automations`: the user's schedule surface (D-367). Filesystem management over the state root's
/// `automations.json`; the daemon does the ticking, opening a goal and submitting the prompt for each due one.
fn run_automations(args: &Args) -> i32 {
    use teamagents_engine::v2::automations::Automations;
    let home = args.state_root.clone().map(PathBuf::from).unwrap_or_else(teamagents_engine::v2_root);
    let mut schedule = match Automations::load(&home) {
        Ok(schedule) => schedule,
        Err(error) => {
            eprintln!("automations: {error}");
            return 1;
        }
    };
    let now = teamagents_core::models::now();
    let verb = args.positional.as_deref().unwrap_or("list");
    match verb {
        "list" | "ls" => {
            let rows = schedule.rows();
            if args.exec_json {
                let report = serde_json::json!({"state_root": home.to_string_lossy(), "now": now, "automations": rows});
                println!("{}", serde_json::to_string(&report).unwrap_or_else(|_| "{}".into()));
            } else if rows.is_empty() {
                println!("no automations for {}", home.display());
            } else {
                println!("{} automation(s) in {}", rows.len(), home.display());
                for row in &rows {
                    let every = row["every_secs"].as_u64().unwrap_or(0);
                    println!(
                        "  {}  {:<7}  every {}m  next in {}s  {}",
                        row["id"].as_str().unwrap_or(""),
                        if row["enabled"].as_bool().unwrap_or(false) { "enabled" } else { "paused" },
                        every / 60,
                        (row["next_at"].as_f64().unwrap_or(0.0) - now).max(0.0) as u64,
                        row["name"].as_str().unwrap_or("")
                    );
                }
            }
            0
        }
        "add" => {
            let Some(minutes) = args.every_minutes else {
                eprintln!("automations add needs --every MINUTES");
                return 2;
            };
            let Some(prompt) = args.prompt.as_deref() else {
                eprintln!("automations add needs --prompt TEXT");
                return 2;
            };
            if minutes == 0 {
                eprintln!("automations add: --every must be at least one minute");
                return 2;
            }
            match schedule.add(args.session_name.as_deref().unwrap_or(""), prompt, minutes * 60, now) {
                Ok(entry) => {
                    if args.exec_json {
                        let report = serde_json::json!({"state_root": home.to_string_lossy(), "id": entry.id,
                                                        "name": entry.name, "every_secs": entry.every_secs, "next_at": entry.next_at});
                        println!("{}", serde_json::to_string(&report).unwrap_or_else(|_| "{}".into()));
                    } else {
                        println!(
                            "created automation {} (every {}m; first run in {}s)",
                            entry.id, minutes, entry.every_secs
                        );
                    }
                    0
                }
                Err(error) => {
                    eprintln!("automations add: {error}");
                    1
                }
            }
        }
        "remove" => match args.approval_id.as_deref() {
            Some(id) if args.confirmed => match schedule.remove(id) {
                Ok(entry) => {
                    println!("removed automation {} ({})", entry.id, entry.name);
                    0
                }
                Err(error) => {
                    eprintln!("automations remove: {error}");
                    1
                }
            },
            Some(id) => {
                eprintln!("removing automation {id} is deliberate: add --yes");
                2
            }
            None => {
                eprintln!("automations remove needs --id ID");
                2
            }
        },
        "pause" | "resume" => match args.approval_id.as_deref() {
            Some(id) => match schedule.set_enabled(id, verb == "resume", now) {
                Ok(entry) => {
                    println!(
                        "automation {} {}",
                        entry.id,
                        if entry.enabled { "enabled (next run one period out)" } else { "paused" }
                    );
                    0
                }
                Err(error) => {
                    eprintln!("automations {verb}: {error}");
                    1
                }
            },
            None => {
                eprintln!("automations {verb} needs --id ID");
                2
            }
        },
        other => {
            eprintln!(
                "automations: unknown command {other:?}; use `teamagents automations [list]`, \
                 `automations add --every MINUTES --prompt TEXT [--name NAME]`, `automations pause|resume --id ID`, \
                 `automations remove --id ID --yes`"
            );
            2
        }
    }
}

/// `teamagents sessions`: the picker and lifetime of named sessions (D-364).
///
/// It is filesystem management over the base directory — each named session is its own state root, so A33's
/// one-coordinator-per-root rule is untouched and several sessions can run at once. `list` never opens another
/// session's database (only: does its directory hold a `session.sqlite`, how big, is a daemon answering), and
/// `archive`/`delete` refuse a session whose daemon is live (the socket is the address).
fn run_sessions(args: &Args) -> i32 {
    use teamagents_engine::v2::sessions::{facts, Registry, DEFAULT_ID};
    let home = args.state_root.clone().map(PathBuf::from).unwrap_or_else(teamagents_engine::v2_root);
    let mut registry = match Registry::load(&home) {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!("sessions: {error}");
            return 1;
        }
    };
    let verb = args.positional.as_deref().unwrap_or("list");
    match verb {
        "list" | "ls" => {
            let rows: Vec<serde_json::Value> = registry
                .rows()
                .into_iter()
                .map(|(entry, dir)| {
                    let mut row = facts(&dir);
                    row["id"] = serde_json::json!(entry.id);
                    row["name"] = serde_json::json!(entry.name);
                    row["path"] = serde_json::json!(dir.to_string_lossy());
                    row["archived"] = serde_json::json!(entry.archived);
                    row["default"] = serde_json::json!(entry.id == DEFAULT_ID);
                    row
                })
                .collect();
            if args.exec_json {
                let report = serde_json::json!({"state_root": home.to_string_lossy(), "sessions": rows});
                println!("{}", serde_json::to_string(&report).unwrap_or_else(|_| "{}".into()));
            } else {
                println!("state root {}: {} session(s)", home.display(), rows.len());
                for row in &rows {
                    let state = if row["archived"].as_bool().unwrap_or(false) {
                        "archived".to_string()
                    } else if row["live"].as_bool().unwrap_or(false) {
                        "live".to_string()
                    } else if row["has_database"].as_bool().unwrap_or(false) {
                        "idle".to_string()
                    } else {
                        "empty".to_string()
                    };
                    println!(
                        "  {}  {:<10}  {:<24}  {}",
                        row["id"].as_str().unwrap_or(""),
                        state,
                        row["name"].as_str().unwrap_or(""),
                        row["path"].as_str().unwrap_or("")
                    );
                }
            }
            0
        }
        "new" => match registry.new_session(args.session_name.as_deref().unwrap_or("")) {
            Ok(entry) => {
                if args.exec_json {
                    let report = serde_json::json!({"state_root": home.to_string_lossy(), "id": entry.id,
                                                    "name": entry.name, "path": home.join(&entry.path).to_string_lossy()});
                    println!("{}", serde_json::to_string(&report).unwrap_or_else(|_| "{}".into()));
                } else {
                    println!("created session {} ({})", entry.id, home.join(&entry.path).display());
                    println!("attach with `teamagents --state-root {} --session {}`", home.display(), entry.id);
                }
                0
            }
            Err(error) => {
                eprintln!("sessions new: {error}");
                1
            }
        },
        "fork" => match args.approval_id.as_deref() {
            Some(id) => match registry.fork(id, args.session_name.as_deref().unwrap_or("")) {
                Ok(entry) => {
                    if args.exec_json {
                        let report = serde_json::json!({"state_root": home.to_string_lossy(), "id": entry.id,
                                                        "name": entry.name, "path": home.join(&entry.path).to_string_lossy(),
                                                        "forked_from": id});
                        println!("{}", serde_json::to_string(&report).unwrap_or_else(|_| "{}".into()));
                    } else {
                        println!("forked session {id} into {} ({})", entry.id, home.join(&entry.path).display());
                        println!("attach with `teamagents --state-root {} --session {}`", home.display(), entry.id);
                    }
                    0
                }
                Err(error) => {
                    eprintln!("sessions fork: {error}");
                    1
                }
            },
            None => {
                eprintln!("sessions fork needs --id ID (the source session; see `teamagents sessions`)");
                2
            }
        },
        "archive" => match args.approval_id.as_deref() {
            Some(id) => match registry.archive(id) {
                Ok(dir) => {
                    println!("archived session {id} to {}", dir.display());
                    0
                }
                Err(error) => {
                    eprintln!("sessions archive: {error}");
                    1
                }
            },
            None => {
                eprintln!("sessions archive needs --id ID (see `teamagents sessions`)");
                2
            }
        },
        "rename" => match (args.approval_id.as_deref(), args.session_name.as_deref()) {
            (Some(id), Some(name)) => match registry.rename(id, name) {
                Ok(entry) => {
                    println!("renamed session {} to {}", entry.id, entry.name);
                    0
                }
                Err(error) => {
                    eprintln!("sessions rename: {error}");
                    1
                }
            },
            (None, _) => {
                eprintln!("sessions rename needs --id ID (see `teamagents sessions`)");
                2
            }
            (_, None) => {
                eprintln!("sessions rename needs --name NAME");
                2
            }
        },
        "restore" => match args.approval_id.as_deref() {
            Some(id) => match registry.restore(id) {
                Ok(entry) => {
                    println!("restored session {} ({})", entry.id, entry.name);
                    0
                }
                Err(error) => {
                    eprintln!("sessions restore: {error}");
                    1
                }
            },
            None => {
                eprintln!("sessions restore needs --id ID (see `teamagents sessions`)");
                2
            }
        },
        "search" => {
            let Some(query) = args.query.as_deref() else {
                eprintln!("sessions search needs --query TEXT");
                return 2;
            };
            let limit = args.limit.unwrap_or(20).max(1) as usize;
            match teamagents_engine::v2::sessions::search(&home, query, limit) {
                Ok((hits, skipped)) => {
                    if args.exec_json {
                        let report = serde_json::json!({"state_root": home.to_string_lossy(), "query": query,
                                                        "hits": hits, "skipped": skipped});
                        println!("{}", serde_json::to_string(&report).unwrap_or_else(|_| "{}".into()));
                    } else if hits.is_empty() {
                        println!("no match for {query:?} in {}", home.display());
                    } else {
                        for hit in &hits {
                            println!(
                                "  {} {}:{}  {}",
                                hit["session_id"].as_str().unwrap_or(""),
                                hit["instance_id"].as_str().unwrap_or(""),
                                hit["idx"],
                                hit["snippet"].as_str().unwrap_or("")
                            );
                        }
                    }
                    for skip in &skipped {
                        eprintln!(
                            "sessions search: {} skipped: {}",
                            skip["session_id"].as_str().unwrap_or(""),
                            skip["reason"].as_str().unwrap_or("")
                        );
                    }
                    0
                }
                Err(error) => {
                    eprintln!("sessions search: {error}");
                    1
                }
            }
        }
        "delete" => match args.approval_id.as_deref() {
            Some(id) if args.confirmed => match registry.delete(id) {
                Ok(dir) => {
                    println!("deleted session {id} ({})", dir.display());
                    0
                }
                Err(error) => {
                    eprintln!("sessions delete: {error}");
                    1
                }
            },
            Some(id) => {
                eprintln!("deleting session {id} removes its directory and history: add --yes to mean it");
                2
            }
            None => {
                eprintln!("sessions delete needs --id ID (see `teamagents sessions`)");
                2
            }
        },
        other => {
            eprintln!(
                "sessions: unknown command {other:?}; use `teamagents sessions [list]`, `sessions new [--name NAME]`, \
                 `sessions fork --id ID [--name NAME]`, `sessions rename --id ID --name NAME`, `sessions restore --id ID`, \
                 `sessions search --query TEXT`, `sessions archive --id ID`, `sessions delete --id ID --yes`"
            );
            2
        }
    }
}

fn run_instances(args: &Args) -> i32 {
    use teamagents_engine::v2::intervene::{InterventionCommand, InterventionOptions};
    let state_root = args.state_root.clone().map(PathBuf::from).unwrap_or_else(teamagents_engine::v2_root);
    let command = match (args.positional.as_deref().unwrap_or("list"), args.approval_id.clone()) {
        ("list", None) => InterventionCommand::ListInstances,
        ("pause", Some(id)) => InterventionCommand::Pause { id },
        ("resume", Some(id)) => InterventionCommand::Resume { id },
        ("interrupt", Some(id)) => InterventionCommand::Interrupt { id },
        ("terminate", Some(id)) => InterventionCommand::Terminate { id },
        // D-252: the worktree member's branch, brought into the session's own tree
        ("merge", Some(id)) => InterventionCommand::Merge { id },
        ("list", Some(_)) => {
            eprintln!("instances list takes no --id; use `instances pause|resume|interrupt|terminate|merge --id ID`");
            return 2;
        }
        (verb @ ("pause" | "resume" | "interrupt" | "terminate" | "merge"), None) => {
            eprintln!("instances {verb} needs --id ID (see `teamagents instances` for the ids)");
            return 2;
        }
        (other, _) => {
            eprintln!(
                "instances: unknown command {other:?}; use `teamagents instances [list]`, \
                 `instances pause|resume|interrupt|terminate --id ID [--yes]`"
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
/// `teamagents surface`: the read-only record of what each model request was offered (D-349, the decision
/// D-341 made for D-143): the tool *names* a request carried and whether the surface check authorized that set.
fn run_surface(args: &Args) -> i32 {
    use teamagents_engine::v2::surfaces::{SurfaceCommand, SurfaceOptions};
    let state_root = args.state_root.clone().map(PathBuf::from).unwrap_or_else(teamagents_engine::v2_root);
    if let Some(other) = args.positional.as_deref() {
        eprintln!(
            "surface: unknown command {other:?}; use `teamagents surface [--id INSTANCE] [--json]` — \n\
             the read-only record of what a request was offered (`--id` resolves by prefix, see `instances`)"
        );
        return 2;
    }
    teamagents_engine::v2::surfaces::run(SurfaceOptions {
        socket: state_root.join("daemon.sock"),
        command: SurfaceCommand::List { instance: args.approval_id.clone() },
        json_out: args.exec_json,
    })
}

fn run_runners(args: &Args) -> i32 {
    use teamagents_engine::v2::runners::{RunnersCommand, RunnersOptions};
    let state_root = args.state_root.clone().map(PathBuf::from).unwrap_or_else(teamagents_engine::v2_root);
    // `--service` *is* the stop action (the listing already shows what a group holds), so it may stand in for
    // the subcommand; it never combines with `list`, which reads.
    let verb = args.positional.as_deref().unwrap_or(if args.service_stop { "stop" } else { "list" });
    let command = match (verb, args.approval_id.clone()) {
        ("list", None) => RunnersCommand::List,
        ("list", Some(_)) => {
            eprintln!("runners list takes no --id; use `runners stop --id JOB`");
            return 2;
        }
        ("stop", job) => RunnersCommand::Stop { job_id: job, service: args.service_stop },
        (other, _) => {
            eprintln!(
                "runners: unknown command {other:?}; use `teamagents runners [list]`, \
                 `runners stop [--id JOB]` or `runners stop --service --yes [--id JOB]`"
            );
            return 2;
        }
    };
    teamagents_engine::v2::runners::run(RunnersOptions {
        state_root,
        command,
        json_out: args.exec_json,
        confirmed: args.confirmed,
    })
}

/// `teamagents artifacts`: the census and the on-demand half of §4.3's collection (D-253).
///
/// Deliberately not a client of the running session: the root that needs collecting is the one whose last
/// driver never boots again. `gc` takes §6.1's coordinator lock itself, so it refuses while a session is live.
fn run_artifacts(args: &Args) -> i32 {
    use teamagents_engine::v2::artifacts::{ArtifactsCommand, ArtifactsOptions};
    let state_root = args.state_root.clone().map(PathBuf::from).unwrap_or_else(teamagents_engine::v2_root);
    let command = match args.positional.as_deref().unwrap_or("list") {
        "list" => ArtifactsCommand::List,
        "gc" => ArtifactsCommand::Gc,
        other => {
            eprintln!("artifacts: unknown command {other:?}; use `teamagents artifacts [list]` or `artifacts gc`");
            return 2;
        }
    };
    teamagents_engine::v2::artifacts::run(ArtifactsOptions { state_root, command, json_out: args.exec_json })
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
        return Err("exec needs a prompt: teamagents exec [--json] [--timeout SEC] [--check CMD] [--] \"…\" (or - to read it from stdin; use -- before a prompt that starts with a dash)".into());
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
    let mut args = parse_args();
    if let Err(error) = resolve_session(&mut args) {
        eprintln!("teamagents: {error}");
        std::process::exit(2);
    }
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
        Some("goals") => run_goals(&args),
        Some("mcp") => run_mcp(&args),
        Some("instances") => run_instances(&args),
        Some("sessions") => run_sessions(&args),
        Some("automations") => run_automations(&args),
        Some("tasks") => run_tasks(&args),
        Some("runners") => run_runners(&args),
        Some("surface") => run_surface(&args),
        Some("artifacts") => run_artifacts(&args),
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
