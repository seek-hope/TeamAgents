//! D-527: command help and accepted options share one command inventory.
use super::Args;

pub struct Command {
    pub name: &'static str,
    pub verb: &'static str,
    pub options: &'static str,
    pub required: &'static str,
    pub summary: &'static str,
}

macro_rules! commands {
    ($(($name:literal, $verb:literal, $options:literal, $required:literal, $summary:literal)),+ $(,)?) => {
        pub const COMMANDS: &[Command] = &[$(Command { name: $name, verb: $verb, options: $options, required: $required, summary: $summary }),+];
    };
}

commands! {
    ("", "", "--cwd DIR --model KEY -c KEY=VALUE", "", "Open the TUI; resume the latest session in this workspace or create one."),
    ("exec", "", "--cwd DIR --model KEY -c KEY=VALUE --json --stream-json --timeout SEC --check CMD --accept ID=CMD --attach PATH", "", "Run one PROMPT, or read stdin with -. Use -- before literal prompts. --check runs after completion; --accept gates runtime completion."),
    ("daemon", "", "--cwd DIR --model KEY -c KEY=VALUE", "", "Run a session daemon in the foreground."),
    ("daemon", "stop", "--stop", "", "Stop the selected daemon and its work."),
    ("init", "", "", "", "Write initial configuration and prepare the state root."),
    ("doctor", "", "--cwd DIR -c KEY=VALUE", "", "Check configuration, credentials, state root and host."),
    ("doctor", "environment", "--cwd DIR --json", "", "Inspect host and configured tools without running repairs."),
    ("version", "", "", "", "Print the version; --version and -V are aliases."),
    ("status", "", "--json", "", "Read the session checkpoint, model, usage, limits and MCP state."),
    ("history", "", "--id INSTANCE --limit N --json", "", "Read the newest 1..1000 entries; defaults to Leader and 400 entries."),
    ("search_tree", "", "--json", "", "Read the session exploration tree."),
    ("surface", "", "--id INSTANCE --cwd DIR --json", "", "Inspect the tools and authorizations offered to an instance."),
    ("authority", "list", "--cwd DIR --json", "", "List grants and pending capability requests."),
    ("authority", "requests", "--cwd DIR --json", "", "List pending capability requests."),
    ("authority", "grant", "--cwd DIR --json --subject ID --action ACTION --scope SCOPE --parent GRANT --delegable", "--subject --action --scope", "Grant scoped authority; parent and delegable govern attenuation."),
    ("authority", "revoke", "--cwd DIR --json --grant ID", "--grant", "Revoke a grant and its descendants."),
    ("authority", "approve", "--cwd DIR --json --request ID", "--request", "Approve a capability request."),
    ("authority", "deny", "--cwd DIR --json --request ID", "--request", "Deny a capability request."),
    ("approvals", "list", "--cwd DIR --json", "", "List operation approvals."),
    ("approvals", "approve", "--cwd DIR --json --id ID", "--id", "Approve one operation bound to its arguments."),
    ("approvals", "deny", "--cwd DIR --json --id ID", "--id", "Deny one operation."),
    ("instances", "list", "--cwd DIR --json", "", "List session instances."),
    ("instances", "send", "--cwd DIR --json --id ID --prompt TEXT --after-turn", "--id --prompt", "Send to a member; --after-turn defers delivery until its current turn ends."),
    ("instances", "model", "--cwd DIR --json --role leader|workers --model KEY --effort LEVEL --follow", "--role", "Select a Leader model/effort or pin all Workers to a key or follow the Leader."),
    ("instances", "resume", "--cwd DIR --json --id ID", "--id", "Resume a parked instance."),
    ("instances", "pause", "--cwd DIR --json --id ID", "--id", "Pause an instance."),
    ("instances", "interrupt", "--cwd DIR --json --id ID", "--id", "Interrupt its running model turn."),
    ("instances", "terminate", "--cwd DIR --json --id ID --yes", "--id --yes", "Terminate an instance and handle its workspace and open work."),
    ("instances", "merge", "--cwd DIR --json --id ID", "--id", "Merge a git_worktree member's branch."),
    ("tasks", "list", "--cwd DIR --json", "", "List task outcomes; an outcome does not prove a process stopped."),
    ("tasks", "cancel", "--cwd DIR --json --id ID", "--id", "Cancel a task and release result waiters; this does not stop its command."),
    ("inputs", "paths", "--query PREFIX --json", "", "Complete workspace attachment paths."),
    ("inputs", "list", "--recipient ID --after N --limit N --json", "", "List pending inputs with revisions; recipient defaults to Leader."),
    ("inputs", "edit", "--id ID --revision N --prompt TEXT --recipient ID --json", "--id --revision --prompt", "Revise pending text with optimistic revision checking."),
    ("inputs", "cancel", "--id ID --revision N --recipient ID --json", "--id --revision", "Cancel pending delivery with optimistic revision checking."),
    ("work_scopes", "list", "--cwd DIR --json", "", "List runtime work scopes."),
    ("work_scopes", "open", "--cwd DIR --json --id ID --attach INSTANCE --check ID=COMMAND --deadline MIN", "--id", "Open work with runtime checks and an optional deadline in minutes."),
    ("work_scopes", "cancel", "--cwd DIR --json --id ID", "--id", "Cancel work and release instances parked by its refusal."),
    ("sessions", "list", "--json", "", "List sessions in the registry; ls is an alias."),
    ("sessions", "new", "--name NAME --cwd DIR --json", "", "Create a session bound to this workspace without starting work."),
    ("sessions", "fork", "--id ID --name NAME --json", "--id", "Fork a stopped session into a fresh execution identity."),
    ("sessions", "rename", "--id ID --name NAME --json", "--id --name", "Rename a session."),
    ("sessions", "archive", "--id ID --json", "--id", "Archive a session; a running daemon is refused."),
    ("sessions", "restore", "--id ID --json", "--id", "Restore an archived session."),
    ("sessions", "search", "--query TEXT --limit N --json", "--query", "Search stored conversations without starting work."),
    ("sessions", "history", "--id ID --before N --limit N --json", "", "Page stored history; ID defaults to default."),
    ("sessions", "export", "--id ID --output FILE --format json|markdown --json", "--output", "Export a complete snapshot without overwriting FILE; ID defaults to default."),
    ("sessions", "open", "--id ID --json", "", "Open the saved workspace and always print a JSON socket report; ID defaults to default."),
    ("sessions", "delete", "--id ID --yes --stop --json", "--id --yes", "Delete session history; --stop first stops its daemon and work. No dry-run mode."),
    ("sessions", "prune", "--days N --dry-run --yes --cwd DIR --json", "", "Preview expired archived sessions with --dry-run, or delete them with --yes."),
    ("automations", "list", "--cwd DIR --json", "", "List periodic work; ls is an alias."),
    ("automations", "add", "--cwd DIR --every MIN --prompt TEXT --name NAME --json", "--every --prompt", "Create periodic work."),
    ("automations", "pause", "--cwd DIR --id ID --json", "--id", "Pause periodic work."),
    ("automations", "resume", "--cwd DIR --id ID --json", "--id", "Re-arm periodic work one period out."),
    ("automations", "remove", "--cwd DIR --id ID --yes --json", "--id --yes", "Remove periodic work."),
    ("runners", "list", "--cwd DIR --json", "", "List durable shell runners."),
    ("runners", "stop", "--cwd DIR --id JOB --service --yes --json", "", "Retire runners; --service --yes stops a group left by a settled command."),
    ("artifacts", "list", "--cwd DIR --json", "", "List stored artifacts and presence."),
    ("artifacts", "gc", "--cwd DIR --json", "", "Collect unreferenced artifacts."),
    ("local-models", "list", "--model KEY --json", "--model", "List model states from the local router bound to a user profile."),
    ("local-models", "load", "--model KEY --yes --json", "--model --yes", "Request loading the selected profile's model on its local router."),
    ("local-models", "unload", "--model KEY --yes --json", "--model --yes", "Request unloading the selected profile's model on its local router."),
    ("catalog", "list", "--name SOURCE --json", "", "List cached directory metadata, including missing models."),
    ("catalog", "refresh", "--name SOURCE --json", "", "Refresh user-bound model directories without inference."),
    ("catalog", "rollback", "--name SOURCE --json", "--name", "Publish the previous directory snapshot as a new version."),
    ("mcp", "list", "--cwd DIR", "", "List configured MCP services, tools and OAuth state."),
    ("mcp", "add", "--name NAME --transport stdio|http --command CMD --arg ARG --url URL --server NAME --oauth --oauth-client-id ID", "--name", "Add a host MCP binding. Repeat --arg for exact argv, including dash-leading values."),
    ("mcp", "remove", "--name NAME", "--name", "Remove an MCP binding."),
    ("mcp", "login", "--name NAME --cwd DIR --no-browser --paste --timeout SEC", "--name", "Authenticate OAuth; --paste reads the redirect instead of waiting for a callback."),
    ("mcp", "logout", "--name NAME", "--name", "Remove the stored credential."),
    ("jobs-runner", "", "", "", "Internal durable shell runner; requires its job directory.")
}

fn locators(command: &Command) -> &'static str {
    match (command.name, command.verb) {
        ("version" | "catalog" | "local-models" | "mcp" | "jobs-runner", _) | ("doctor", "environment") => "",
        ("sessions", _) => "--state-root PATH",
        _ => "--state-root PATH --session ID",
    }
}

pub fn verb(args: &Args) -> &str {
    match args.command.as_deref().unwrap_or("") {
        "exec" | "jobs-runner" | "" => "",
        "daemon" => {
            if args.daemon_stop {
                "stop"
            } else {
                ""
            }
        }
        "authority" => args.positional.as_deref().unwrap_or_else(|| {
            if args.grant.is_some() {
                "revoke"
            } else if args.request.is_some() {
                "approve"
            } else if args.subject.is_some() || args.action.is_some() || args.scope.is_some() {
                "grant"
            } else {
                "list"
            }
        }),
        "approvals" => {
            args.positional.as_deref().unwrap_or(if args.approval_id.is_some() { "approve" } else { "list" })
        }
        "runners" => args.positional.as_deref().unwrap_or(if args.service_stop { "stop" } else { "list" }),
        "sessions" | "automations" => match args.positional.as_deref().unwrap_or("list") {
            "ls" => "list",
            v => v,
        },
        name if COMMANDS.iter().any(|c| c.name == name && c.verb == "list") => {
            args.positional.as_deref().unwrap_or("list")
        }
        _ => args.positional.as_deref().unwrap_or(""),
    }
}

pub fn selected(args: &Args) -> Result<&'static Command, String> {
    let name = args.command.as_deref().unwrap_or("");
    let verb = verb(args);
    if (name.is_empty() || name == "daemon") && args.positional.is_some() {
        return Err(format!("{:?} is not an entry point here; this entry point takes no positional argument. Use teamagents exec PROMPT", args.positional.as_deref().unwrap_or("")));
    }
    COMMANDS
        .iter()
        .find(|c| c.name == name && c.verb == verb)
        .ok_or_else(|| format!("unknown command {name} {verb}; use teamagents {name} --help"))
}

pub fn validate(args: &Args, flags: &[String], help: bool) -> Result<(), String> {
    let command = selected(args)?;
    let accepts = |flag: &str| {
        command.options.split_whitespace().chain(locators(command).split_whitespace()).any(|f| f == flag)
        // Compatibility only: neither flag provides a second execution mode.
        || (flag == "--full-auto" && matches!((command.name, command.verb), ("" | "exec", "") | ("daemon", "")))
        || (flag == "--execution" && command.name == "mcp" && command.verb == "add")
    };
    for flag in flags {
        if !accepts(flag) {
            if command.name == "init" {
                return Err(format!("init takes no other arguments beyond --state-root/--session: unexpected {flag}"));
            }
            return Err(format!(
                "{} {} does not take {flag}; use teamagents {} {} --help",
                command.name, command.verb, command.name, command.verb
            ));
        }
    }
    if help {
        return Ok(());
    }
    for flag in command.required.split_whitespace() {
        if !flags.iter().any(|given| given == flag) {
            return Err(format!(
                "{} {} needs {flag}; use teamagents {} {} --help",
                command.name, command.verb, command.name, command.verb
            ));
        }
    }
    if (command.name, command.verb) == ("instances", "model") {
        match args.role.as_deref() {
            Some("leader") if !args.follow && (args.model.is_some() || args.effort.is_some()) => {},
            Some("workers") if args.effort.is_none() && (args.model.is_some() != args.follow) => {},
            _ => return Err("use --role leader with --model or --effort, or --role workers with exactly one of --model KEY and --follow".into()),
        }
    }
    if command.name == "mcp" {
        if command.verb == "add" {
            let forbidden: &[&str] = match args.mcp_transport.as_deref().unwrap_or("stdio") {
                "stdio" => &["--url", "--oauth", "--oauth-client-id"],
                "http" => &["--command", "--arg"],
                other => return Err(format!("unsupported MCP transport {other:?}; use stdio or http")),
            };
            if let Some(flag) = flags.iter().find(|flag| forbidden.contains(&flag.as_str())) {
                let needed = if args.mcp_transport.as_deref().unwrap_or("stdio") == "stdio" { "http" } else { "stdio" };
                return Err(format!("mcp add: {flag} needs --transport {needed}"));
            }
        }
        if command.verb == "login" && args.mcp_login_paste && args.mcp_login_timeout.is_some() {
            return Err("mcp login --paste does not take --timeout; no callback is awaited".into());
        }
    }
    if args.cwd.is_some()
        && (args.state_root.is_some() || args.session.is_some())
        && matches!(
            command.name,
            "authority" | "approvals" | "instances" | "tasks" | "work_scopes" | "runners" | "surface" | "automations"
        )
    {
        return Err("--cwd only selects a workspace session here; it does not apply with --state-root/--session".into());
    }
    if command.name == "runners" && args.confirmed && !args.service_stop {
        return Err("runners --yes requires stop --service".into());
    }
    Ok(())
}

pub fn help(args: &Args) -> Result<(), String> {
    let selected = selected(args)?;
    let family = args.positional.is_none() && !args.daemon_stop;
    for command in COMMANDS.iter().filter(|c| c.name == selected.name && (family || c.verb == selected.verb)) {
        let verb = if command.name == "daemon" && command.verb == "stop" { "--stop" } else { command.verb };
        println!("teamagents {} {} [OPTIONS]\n  {}", command.name, verb, command.summary);
        if !command.required.is_empty() {
            println!("  Required: {}", command.required);
        }
        println!("  Options: {} {} --help (-h)\n", command.options, locators(command));
    }
    if selected.name == "exec" {
        println!("Repeat --attach, --check and --accept as needed. --timeout uses seconds; 0 is refused.\nAttachments: eight files, 1 MiB/text, 5 MiB/image, 16 MiB total; symlinks refused.\n--json prints one report; --stream-json prints JSONL events followed by the report.\nExit codes: 0 completed, 1 failed/unfinished, 3 approval required, 124 timeout, 2 usage.");
    }
    Ok(())
}
