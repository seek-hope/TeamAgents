use crate::{
    daemon,
    protocol::{field, is_terminal_str, outcome_label, preview, sanitize_terminal, Client, QuestionTracker},
    Result,
};
use serde_json::{json, Value};
use std::env;
use std::ffi::OsString;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::sync::{mpsc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(500);
const HELP: &str = "TeamAgents — persistent agent sessions

  teamagents [chat] [--review] [--plain] Submit prompts and receive final reports (TUI on TTY)
  teamagents tui [--review]              Launch the interactive terminal interface
  teamagents exec [OPTIONS] TEXT         Submit a task (use - or pipe stdin for text)
  teamagents members | tasks             List session members or tasks
  teamagents status TASK_ID              Inspect progress, result and pending action
  teamagents answer ROOT_ID TEXT          Answer a root clarification and resume work
  teamagents cancel TASK_ID              Request cancellation
  teamagents delete ROOT_ID              Delete a settled or idle session tree
  teamagents approve | deny APPROVAL_ID  Decide one pending action, not a whole task
  teamagents auth login | status | logout  Manage ChatGPT subscription sign-in
  teamagents stop                        Stop an idle daemon; keep the session
  teamagents help | version

Options may appear before or after the command. Use -- before literal arguments.
  --session PATH       Session directory (default: per workspace under XDG state)
  --workspace PATH     Workspace (default: current directory)
  --agent NAME         Recipient for a new task or chat (default: default)
  --json               Print machine-readable JSON
  --plain              Use the line-oriented chat REPL even on a terminal (chat)
  --detach             Return the submission receipt without waiting (exec)
  --timeout SECONDS    Limit the wait after submission (exec waits indefinitely by default)
  --review             Require approval for each shell command and edit requested by
                       the model on this root or its children (exec, chat, and tui)
  --check COMMAND      Run this exact trusted command at proposed completion (exec).
                       A failure returns diagnostics for repair, up to 3 check attempts
                       within the root's shared model-turn budget.

By default, agents work unattended in a workspace sandbox with network access for
package installs and other project work. Shells cannot read private host files
outside the workspace or inherit the daemon's provider key. Use a trusted workspace.
Leaving or timing out keeps work running. Use status to reconnect and tasks to find
pending approvals. When a root needs clarification, exec prints its question and exits.
Use /model in chat to select the model and reasoning effort for new tasks.
Use answer ROOT_ID TEXT to continue. Final reports distinguish claims from observed results.";

const CHAT_HELP: &str = "Enter a task, or reply to a single waiting question for your selected agent.
Use /answer to select a question or /new to start separate work.
/to NAME   /members   /tasks   /status TASK_ID   /cancel TASK_ID
/answer ROOT_ID TEXT   /new TEXT   /approve APPROVAL_ID   /deny APPROVAL_ID   /help   /quit
/model shows the selection for new roots. /model list shows visible ChatGPT plan models.
/model MODEL [EFFORT]   /model effort EFFORT   /model default";

struct GlobalOptions {
    session: Option<PathBuf>,
    workspace: Option<PathBuf>,
    json: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AuthAction {
    Login,
    Status,
    Logout,
}

struct ExecOptions {
    agent: String,
    review: bool,
    check: Option<String>,
    detach: bool,
    timeout: Option<Duration>,
    args: Vec<String>,
}

enum Command {
    Help,
    Version,
    Auth(AuthAction),
    Daemon,
    Chat { agent: String, review: bool, plain: bool },
    Tui { agent: String, review: bool },
    Exec(ExecOptions),
    Members,
    Tasks,
    Status { id: String },
    Answer { root_id: String, text: String },
    Cancel { id: String },
    Delete { id: String },
    Approve { id: String },
    Deny { id: String },
    Stop,
}

struct Parsed {
    global: GlobalOptions,
    command: Command,
}

fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Parsed> {
    let mut session = None;
    let mut workspace = None;
    let mut agent = String::from("default");
    let mut json = false;
    let mut detach = false;
    let mut timeout = None;
    let mut review = false;
    let mut plain = false;
    let mut check = None;
    let mut cmd_name = String::new();
    let mut pos_args: Vec<String> = Vec::new();
    let mut args = args.into_iter();
    let mut literal = false;
    let mut info = None;

    while let Some(arg) = args.next() {
        let word = arg.to_str().ok_or("command and prompt arguments must be UTF-8")?;
        if literal {
            if cmd_name.is_empty() {
                cmd_name = word.into();
            } else {
                pos_args.push(word.into());
            }
            continue;
        }
        let mut value = || args.next().filter(|v| !v.is_empty()).ok_or_else(|| format!("{word} requires a value"));
        let text = |v: OsString| v.into_string().map_err(|_| format!("{word} requires UTF-8 text"));
        match word {
            "--" => literal = true,
            "--session" => session = Some(value()?.into()),
            "--workspace" => workspace = Some(value()?.into()),
            "--agent" => agent = text(value()?)?,
            "--check" => check = Some(text(value()?)?),
            "--timeout" => {
                let seconds = text(value()?)?.parse().map_err(|_| "--timeout requires nonnegative whole seconds")?;
                timeout = Some(Duration::from_secs(seconds));
            }
            "--json" => json = true,
            "--review" => review = true,
            "--plain" => plain = true,
            "--detach" => detach = true,
            "--help" | "-h" => info = Some("help"),
            "--version" | "-V" => info = Some("version"),
            _ if word.starts_with('-') && word != "-" => {
                return Err(format!("unknown option: {word} (use -- for literal text)"))
            }
            _ if cmd_name.is_empty() => cmd_name = word.into(),
            _ => pos_args.push(word.into()),
        }
    }

    let global = GlobalOptions { session, workspace, json };
    if let Some(info) = info {
        return Ok(Parsed { global, command: if info == "help" { Command::Help } else { Command::Version } });
    }
    if cmd_name.is_empty() {
        cmd_name = "chat".into();
    }

    if cmd_name != "exec" && (check.is_some() || detach || timeout.is_some()) {
        return Err("--check, --timeout and --detach apply only to exec".into());
    }
    if review && !matches!(cmd_name.as_str(), "chat" | "exec" | "tui") {
        return Err("--review applies only to exec, chat, and tui".into());
    }
    if plain && cmd_name != "chat" {
        return Err("--plain applies only to chat".into());
    }
    if json && cmd_name == "tui" {
        return Err("--json does not apply to tui".into());
    }
    if agent.trim().is_empty() {
        return Err("--agent requires a name".into());
    }
    if check.as_deref().is_some_and(|s| s.trim().is_empty()) {
        return Err("--check requires a command".into());
    }

    let command = match cmd_name.as_str() {
        "help" => Command::Help,
        "version" => Command::Version,
        "auth" => {
            if pos_args.len() != 1 || !matches!(pos_args[0].as_str(), "login" | "status" | "logout") {
                return Err("use teamagents auth login, status or logout".into());
            }
            if global.json || global.session.is_some() || global.workspace.is_some() || agent != "default" {
                return Err(
                    "auth manages the local user account and does not take session, workspace, agent or JSON options"
                        .into(),
                );
            }
            Command::Auth(match pos_args[0].as_str() {
                "login" => AuthAction::Login,
                "status" => AuthAction::Status,
                "logout" => AuthAction::Logout,
                _ => unreachable!("validated auth action"),
            })
        }
        "exec" => Command::Exec(ExecOptions { agent, review, check, detach, timeout, args: pos_args }),
        "answer" => {
            if pos_args.len() < 2 || pos_args[0].trim().is_empty() || pos_args[1..].iter().all(|s| s.trim().is_empty())
            {
                return Err("answer requires ROOT_ID and nonempty TEXT".into());
            }
            Command::Answer { root_id: pos_args[0].clone(), text: pos_args[1..].join(" ") }
        }
        "cancel" | "delete" | "status" | "approve" | "deny" => {
            if pos_args.len() != 1 || pos_args[0].trim().is_empty() {
                return Err(format!("{cmd_name} requires exactly one ID"));
            }
            let id = pos_args.into_iter().next().unwrap();
            match cmd_name.as_str() {
                "cancel" => Command::Cancel { id },
                "delete" => Command::Delete { id },
                "status" => Command::Status { id },
                "approve" => Command::Approve { id },
                "deny" => Command::Deny { id },
                _ => unreachable!(),
            }
        }
        "chat" | "tui" | "daemon" | "members" | "tasks" | "stop" => {
            if !pos_args.is_empty() {
                return Err(format!("{cmd_name} does not take arguments"));
            }
            match cmd_name.as_str() {
                "chat" => Command::Chat { agent, review, plain },
                "tui" => Command::Tui { agent, review },
                "daemon" => Command::Daemon,
                "members" => Command::Members,
                "tasks" => Command::Tasks,
                "stop" => Command::Stop,
                _ => unreachable!(),
            }
        }
        cmd => return Err(format!("unknown command: {cmd} (see teamagents help)")),
    };

    Ok(Parsed { global, command })
}

fn display_value(value: &Value) -> String {
    value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string())
}

fn append_evidence(out: &mut String, task: &Value) {
    if let Some(receipts) = task["evidence"].as_array() {
        out.push_str("\n  Model reports alone do not establish that tests passed.");
        let commands: Vec<_> = receipts.iter().filter(|r| r["action"]["command"].is_string()).collect();
        if commands.is_empty() {
            out.push_str("\n  No completed command receipts on this task.");
        }
        for r in commands.iter().rev().take(4).rev() {
            let command = preview(r["action"]["command"].as_str().unwrap(), 160);
            let result = preview(r["result"].as_str().unwrap_or(""), 160);
            out.push_str(&format!(
                "\n  Observed {} [{}] {command} — {result}",
                r["action"]["op"].as_str().unwrap_or("command"),
                outcome_label(r["ok"].as_bool())
            ));
        }
        if commands.len() > 4 {
            out.push_str(&format!("\n  {} earlier command receipts omitted (use status --json).", commands.len() - 4));
        }
    }
    if let Some(index) = task["subtree_receipts"].as_array().filter(|i| !i.is_empty()) {
        out.push_str("\n  Subtree receipts (inspect a task ID for details):");
        for r in index.iter().take(8) {
            let label = preview(r["label"].as_str().unwrap_or(""), 160);
            out.push_str(&format!(
                "\n    {} {}: {} [{}]",
                r["task"].as_str().unwrap_or("?"),
                r["agent"].as_str().unwrap_or("?"),
                r["op"].as_str().unwrap_or("?"),
                outcome_label(r["ok"].as_bool())
            ));
            if !label.is_empty() {
                out.push(' ');
                out.push_str(&label);
            }
        }
        if index.len() > 8 {
            out.push_str(&format!("\n    {} more (use status --json).", index.len() - 8));
        }
    }
}

fn task_text(task: &Value) -> String {
    let mut out = format!(
        "{} [{}] {}",
        task["id"].as_str().unwrap_or("?"),
        task["status"].as_str().unwrap_or("?"),
        task["agent"].as_str().unwrap_or("?")
    );
    if let Some(parent) = task["parent"].as_str() {
        out.push_str(&format!(" (child of {parent})"));
    }
    if let (Some(used), Some(budget)) = (task["turns_used"].as_u64(), task["turn_budget"].as_u64()) {
        out.push_str(&format!(" | root turns {used}/{budget}"));
    }
    if let Some(stage) = task["stage_status"].as_str() {
        if task["parent"].is_null() && task["kind"] != "legacy" {
            out.push_str(&format!(
                " | root aggregate: {} | worker stage: {stage}",
                task["status"].as_str().unwrap_or("?")
            ));
        }
    }
    if task["status"] == "input" && task["parent"].is_null() {
        let id = task["id"].as_str().unwrap_or("?");
        let question = task["question"].as_str().unwrap_or("(question unavailable)");
        out.push_str(&format!(
            "\n  Question: {question}\n  Reply with teamagents answer {id} TEXT or /answer {id} TEXT in chat."
        ));
    }
    if let Some(result) = task["result"].as_str() {
        if task["status"] == "done" {
            out.push_str(&format!("\n  Model report: {result}"));
        } else {
            out.push_str(&format!("\n  Outcome: {}", preview(result, 160)));
        }
    }
    if let Some(error) = task["stage_error"].as_str() {
        out.push_str(&format!("\n  Worker error: {}", preview(error, 160)));
    }
    for key in ["policy", "verification", "check", "tool_state"] {
        if let Some(value) = task[key].as_str() {
            out.push_str(&format!("\n  {key}: {value}"));
        }
    }
    if let Some(approval) = task["approval_id"].as_str() {
        out.push_str(&format!("\n  action: {}\n  approval_id: {approval}", task["action"]));
    } else if !task["action"].is_null() {
        out.push_str(&format!("\n  action: {}", task["action"]["op"].as_str().unwrap_or("?")));
    }
    if let Some(tree) = crate::tree_view::render(task) {
        out.push('\n');
        out.push_str(&tree);
    }
    if let Some(leaves) = task["leaf_results"].as_array().filter(|rows| !rows.is_empty()) {
        out.push_str("\n  Leaf reports and outcomes:");
        for leaf in leaves {
            let id = leaf["task"].as_str().unwrap_or("?");
            let agent = leaf["agent"].as_str().unwrap_or("?");
            let status = leaf["status"].as_str().unwrap_or("?");
            let result = preview(leaf["result"].as_str().unwrap_or(""), 160);
            out.push_str(&format!("\n    {id} {agent} [{status}] {result}"));
        }
    }
    append_evidence(&mut out, task);
    out
}

fn render(op: &str, value: &Value, machine: bool) -> String {
    if machine {
        return value.to_string();
    }
    match op {
        "tasks" | "members" => {
            if let Some(rows) = value.as_array() {
                if rows.is_empty() {
                    return format!("No {op}.");
                }
                rows.iter()
                    .map(|row| if op == "tasks" { task_text(row) } else { display_value(&row["name"]) })
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                value.to_string()
            }
        }
        "status" => task_text(value),
        "submit" => format!("Submitted {}", display_value(&value["id"])),
        _ if value["ok"] == true => "OK".into(),
        _ => display_value(value),
    }
}

fn output(line: &str) -> Result<()> {
    let mut out = io::stdout().lock();
    writeln!(out, "{}", sanitize_terminal(line)).and_then(|_| out.flush()).map_err(|e| e.to_string())
}

fn command(client: &Client, op: &str, id: Option<&str>, machine: bool) -> Result<()> {
    let mut body = json!({"op":op});
    if let Some(id) = id {
        body["id"] = json!(id);
    }
    let value = client.call(body)?;
    output(&render(op, &value, machine))
}

fn prompt(args: &[String]) -> Result<String> {
    if args.is_empty() || args == ["-"] {
        if args.is_empty() && io::stdin().is_terminal() {
            return Err("exec needs a prompt (or use - to read stdin)".into());
        }
        let mut text = String::new();
        io::stdin().read_to_string(&mut text).map_err(|e| e.to_string())?;
        Ok(text)
    } else {
        Ok(args.join(" "))
    }
}

fn exec(client: Client, opt: ExecOptions, machine: bool) -> Result<()> {
    let receipt = client.submit(&opt.agent, &prompt(&opt.args)?, opt.review, opt.check.as_deref())?;
    if opt.detach {
        return output(&render("submit", &receipt, machine));
    }
    let id = field(&receipt, "id")?.to_owned();
    if !machine {
        eprintln!("{}", sanitize_terminal(&format!("Submitted {id}. Reconnect with status {id}.")));
        if opt.review {
            eprintln!("Use tasks in another client to review pending actions, including child approvals.");
        }
    }

    // Keep even a stalled status request outside the client's completion deadline.
    // The worker only reads; dropping this client never cancels or re-submits the task.
    let start = Instant::now();
    let (tx, rx) = mpsc::sync_channel(1);
    let query = json!({"op":"status", "id":id});
    thread::spawn(move || loop {
        let status = client.call(query.clone());
        let finished =
            status.as_ref().map_or(true, |v| v["status"].as_str().is_none_or(|s| is_terminal_str(s) || s == "input"));
        if tx.send(status).is_err() || finished {
            break;
        }
        thread::sleep(POLL);
    });
    let mut last = receipt;
    loop {
        let disconnected = || format!("lost status reader for {id}");
        let task = if let Some(timeout) = opt.timeout {
            let remaining = timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(reply) => reply?,
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(disconnected()),
            }
        } else {
            rx.recv().map_err(|_| disconnected())??
        };
        let status = field(&task, "status")?;
        if status == "input" {
            output(&render("status", &task, machine))?;
            return Err(format!("input required; resume with teamagents answer {id} TEXT"));
        }
        if is_terminal_str(status) {
            output(&render("status", &task, machine))?;
            return if status == "done" { Ok(()) } else { Err(format!("task {id} {status}")) };
        }
        if !machine && status == "approval" && last != task {
            eprintln!("{}", sanitize_terminal(&task_text(&task)));
        }
        last = task;
    }
    output(&render(if last.get("status").is_some() { "status" } else { "submit" }, &last, machine))?;
    Err(format!("timed out waiting for {id}; reconnect with status {id}"))
}

fn model_command(client: &Client, args: &str, machine: bool) -> Result<()> {
    let words: Vec<_> = args.split_whitespace().collect();
    let usage = "use /model, /model list, /model MODEL [EFFORT], /model effort EFFORT or /model default";
    let mut body = json!({"op":"model"});
    let effort_value = |s: &str| if s == "default" { Value::Null } else { json!(s) };
    match words.as_slice() {
        [] | ["list"] => (),
        ["default"] => {
            body["model"] = Value::Null;
            body["effort"] = Value::Null;
        }
        ["effort", effort] => body["effort"] = effort_value(effort),
        ["effort" | "list" | "default", ..] => return Err(usage.into()),
        args => {
            let args = args.strip_prefix(&["set"]).unwrap_or(args);
            let (name, effort) = match args {
                [name] => (name, Value::Null),
                [name, effort] => (name, effort_value(effort)),
                _ => return Err(usage.into()),
            };
            body["model"] = json!(name);
            body["effort"] = effort;
        }
    }
    let mut value = client.call(body)?;
    let provider = field(&value, "provider")?;
    if words == ["list"] {
        if provider != "chatgpt" {
            return Err("/model list uses the ChatGPT account catalog. For this API provider, enter /model MODEL [EFFORT] with its model ID.".into());
        }
        let models = crate::model::plan_models()?;
        if !machine {
            return if models.is_empty() {
                output("No visible models for this ChatGPT account.")
            } else {
                output(&models.join("\n"))
            };
        }
        value["models"] = json!(models);
        return output(&value.to_string());
    }
    if machine {
        return output(&value.to_string());
    }
    let fallback = if provider == "chatgpt" { "automatic (first visible plan model)" } else { "unset" };
    let model = value["model"].as_str().unwrap_or(fallback);
    let effort = value["effort"].as_str().unwrap_or("provider default");
    let mut text = format!("New tasks use {provider} | model: {model} | reasoning effort: {effort}");
    if words.is_empty() {
        text.push_str("\n/model MODEL [EFFORT]   /model effort EFFORT   /model default");
        text.push_str(&format!("\nEffort: default, {} (support varies by model).", daemon::EFFORTS.join(", ")));
        if provider == "chatgpt" {
            text.push_str("\nUse /model list to see model IDs for your ChatGPT account.");
        }
    }
    output(&text)
}

fn chat_plain(client: &Client, initial_agent: &str, review: bool, machine: bool) -> Result<()> {
    let mut agent = initial_agent.to_owned();
    if !machine && io::stdin().is_terminal() {
        let mode = if review { "individual action review" } else { "unattended" };
        output(&format!("Chatting with {agent} ({mode}). Enter a task, or /help for commands."))?;
    }
    let tracker = Mutex::new(QuestionTracker::default());
    let show = |task: &Value| {
        if is_terminal_str(field(task, "status")?) {
            // The list omits receipts. Fetch the final status only when the root finishes.
            command(client, "status", Some(field(task, "id")?), machine)
        } else {
            output(&render("status", task, machine))
        }
    };
    tracker.lock().unwrap().update_raw(client.tasks_raw()?, show)?;
    let answer = |id: &str, text: &str| {
        let value = client.answer(id, text)?;
        output(&render("answer", &value, machine))?;
        tracker.lock().unwrap().clear_answered(id);
        Ok(())
    };
    let submit = |agent: &str, text: &str| {
        let receipt = client.submit(agent, text, review, None)?;
        if machine || review {
            output(&render("submit", &receipt, machine))
        } else {
            Ok(())
        }
    };
    let (stop, stopped) = mpsc::channel::<()>();
    thread::scope(|scope| {
        let tracker = &tracker;
        scope.spawn(move || {
            let mut previous_error = None;
            while stopped.recv_timeout(POLL) == Err(mpsc::RecvTimeoutError::Timeout) {
                let error = client.tasks_raw().and_then(|tasks| tracker.lock().unwrap().update_raw(tasks, show)).err();
                if error != previous_error {
                    if let Some(message) = &error {
                        eprintln!("task updates: {}", sanitize_terminal(message));
                    }
                }
                previous_error = error;
            }
        });
        // Dropping the sender on EOF, /quit or an input error wakes and joins the watcher.
        let _stop = stop;
        for line in io::stdin().lock().lines() {
            let line = line.map_err(|e| e.to_string())?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let result = if let Some(slash) = line.strip_prefix('/') {
                let (op, args) = slash.split_once(char::is_whitespace).unwrap_or((slash, ""));
                let args = args.trim_start();
                let mut parts = args.split_whitespace();
                let arg = parts.next();
                match (op, arg, parts.next()) {
                    ("quit", None, None) => break,
                    ("help", None, None) => output(&render("help", &json!(CHAT_HELP), machine)),
                    ("model", _, _) => model_command(client, args, machine),
                    ("to", Some(name), None) => {
                        agent = name.into();
                        output(&if machine {
                            json!({"agent":agent}).to_string()
                        } else {
                            format!("Chatting with {agent}.")
                        })
                    }
                    ("answer", Some(id), Some(_)) => {
                        let (_, text) = args.split_once(char::is_whitespace).unwrap();
                        answer(id, text.trim_start())
                    }
                    ("new", Some(_), _) => submit(&agent, args),
                    ("tasks" | "members", None, None) | ("status" | "cancel" | "approve" | "deny", Some(_), None) => {
                        command(client, op, arg, machine)
                    }
                    _ => Err(format!("invalid /{op} command; see /help")),
                }
            } else {
                let target = tracker.lock().unwrap().reply_target(&agent);
                match target {
                    Ok(Some(id)) => answer(&id, line),
                    Ok(None) => submit(&agent, line),
                    Err(e) => Err(e),
                }
            };
            if let Err(e) = result {
                eprintln!("teamagents: {}", sanitize_terminal(&e));
            }
        }
        Ok(())
    })
}

pub fn run() -> Result<()> {
    run_cli().map_err(|e| sanitize_terminal(&e))
}

fn run_cli() -> Result<()> {
    let Parsed { global, command: cmd } = parse(env::args_os().skip(1))?;
    match cmd {
        Command::Help => return output(&render("help", &json!(HELP), global.json)),
        Command::Version => return output(&render("version", &json!(env!("CARGO_PKG_VERSION")), global.json)),
        Command::Auth(action) => {
            return match action {
                AuthAction::Login => crate::oauth::login(),
                AuthAction::Status => crate::oauth::status(),
                AuthAction::Logout => crate::oauth::logout(),
            };
        }
        _ => {}
    }

    let client = Client::new(global.session.as_deref(), global.workspace.as_deref())?;
    let interactive_tty = io::stdin().is_terminal() && io::stdout().is_terminal();
    match cmd {
        Command::Daemon => daemon::serve(&client.session, &client.workspace),
        Command::Tui { agent, review } => {
            if !interactive_tty {
                return Err(
                    "tui requires an interactive terminal; use `teamagents chat --plain` or `teamagents exec`".into()
                );
            }
            crate::tui::run(client, agent, review)
        }
        Command::Chat { agent, review, plain } => {
            if !global.json && !plain && interactive_tty {
                crate::tui::run(client, agent, review)
            } else {
                chat_plain(&client, &agent, review, global.json)
            }
        }
        Command::Exec(opt) => exec(client, opt, global.json),
        Command::Answer { root_id, text } => {
            let value = client.answer(&root_id, &text)?;
            output(&render("answer", &value, global.json))
        }
        Command::Members => command(&client, "members", None, global.json),
        Command::Tasks => command(&client, "tasks", None, global.json),
        Command::Status { id } => command(&client, "status", Some(&id), global.json),
        Command::Cancel { id } => command(&client, "cancel", Some(&id), global.json),
        Command::Delete { id } => command(&client, "delete", Some(&id), global.json),
        Command::Approve { id } => command(&client, "approve", Some(&id), global.json),
        Command::Deny { id } => command(&client, "deny", Some(&id), global.json),
        Command::Stop => command(&client, "stop", None, global.json),
        Command::Help | Command::Version | Command::Auth(_) => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(args: &[&str]) -> Result<Parsed> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn options_and_literal_prompt_do_not_depend_on_flag_position() {
        let parsed = options(&[
            "--session",
            "s",
            "--json",
            "exec",
            "--workspace",
            "w",
            "--agent",
            "worker",
            "--timeout",
            "2",
            "--check",
            "cargo test",
            "--detach",
            "--",
            "--help",
            "literal",
        ])
        .unwrap();
        assert_eq!(parsed.global.session.as_deref(), Some(std::path::Path::new("s")));
        assert_eq!(parsed.global.workspace.as_deref(), Some(std::path::Path::new("w")));
        assert!(parsed.global.json);
        match parsed.command {
            Command::Exec(opt) => {
                assert!(!opt.review && opt.detach);
                assert_eq!(opt.agent, "worker");
                assert_eq!(opt.timeout, Some(Duration::from_secs(2)));
                assert_eq!(opt.check.as_deref(), Some("cargo test"));
                assert_eq!(prompt(&opt.args).unwrap(), "--help literal");
            }
            _ => panic!("expected exec"),
        }

        assert!(matches!(options(&[]).unwrap().command, Command::Chat { plain: false, .. }));
        assert!(matches!(
            options(&["chat", "--plain", "--review"]).unwrap().command,
            Command::Chat { plain: true, review: true, .. }
        ));
        assert!(matches!(
            options(&["--review", "tui", "--agent", "builder"]).unwrap().command,
            Command::Tui { ref agent, review: true } if agent == "builder"
        ));
        assert!(matches!(
            options(&["delete", "r1"]).unwrap().command,
            Command::Delete { ref id } if id == "r1"
        ));
        for args in [
            &["deny"][..],
            &["delete"],
            &["delete", "r1", "extra"],
            &["answer"],
            &["answer", "t1"],
            &["answer", "t1", " "],
            &["status", "a", "b"],
            &["exec", "--timeout", "-1"],
            &["--check", "x", "tasks"],
            &["tasks", "--review"],
            &["chat", "--timeout", "1"],
            &["exec", "--plain"],
            &["tasks", "--plain"],
            &["tui", "--json"],
            &["tui", "--plain"],
            &["tui", "extra"],
        ] {
            assert!(options(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn chat_shows_child_approval_and_completion_only_when_changed() {
        let finished = json!({"id":"old", "status":"done"});
        let child = json!({"id":"t2", "parent":"t1", "agent":"worker", "status":"approval", "approval_id":"a1", "action":{"op":"shell", "command":"cargo test"}});
        let mut seen = Vec::new();
        let mut show = |v: &Value| {
            seen.push(render("status", v, false));
            Ok(())
        };
        let mut tracker = QuestionTracker::default();
        tracker.update_raw(vec![finished.clone(), child.clone()], &mut show).unwrap();
        tracker.update_raw(vec![finished, child], &mut show).unwrap();
        tracker
            .update_raw(vec![json!({"id":"t2", "parent":"t1", "status":"done", "result":"child report"})], &mut show)
            .unwrap();
        tracker.update_raw(vec![json!({"id":"t1", "status":"done", "result":"all tests passed"})], &mut show).unwrap();
        assert_eq!(seen.len(), 2);
        for s in ["child of t1", "approval_id: a1", "\"command\":\"cargo test\""] {
            assert!(seen[0].contains(s));
        }
        assert!(seen[1].contains("all tests passed"));
    }

    #[test]
    fn natural_reply_never_redirects_a_stale_answer() {
        let mut tracker = QuestionTracker::default();
        let first = json!({"id":"first","agent":"a","status":"input","question":"Which target?"});
        let second = json!({"id":"second","agent":"a","status":"input","question":"Which version?"});
        tracker.update_raw(vec![first.clone(), second], |_| Ok(())).unwrap();
        assert!(tracker.reply_target("a").is_err());
        tracker.update_raw(vec![first, json!({"id":"second","agent":"a","status":"queued"})], |_| Ok(())).unwrap();
        assert!(tracker.reply_target("a").unwrap_err().contains("Nothing was sent"));
        assert!(tracker.reply_target("different agent").is_err());
        assert_eq!(tracker.reply_target("a").unwrap().as_deref(), Some("first"));
        tracker.update_raw(vec![json!({"id":"first","agent":"a","status":"cancelled"})], |_| Ok(())).unwrap();
        assert!(tracker.reply_target("a").is_err());
        assert!(tracker.reply_target("a").unwrap().is_none());
    }

    #[test]
    fn human_receipts_are_bounded_without_hiding_failure_or_changing_json() {
        let commands: Vec<_> = (0..6)
            .map(|i| json!({"action":{"op":"verify","command":format!("check {i}")}, "ok":false,"result":"x".repeat(1000)}))
            .collect();
        let index: Vec<_> = (0..9)
            .map(|i| json!({"task":format!("t{i}"),"agent":"builder","op":"verify","label":"make test","ok":false}))
            .collect();
        let task = json!({"id":"t", "status":"failed", "verification":"failed", "result":"attempts exhausted", "evidence":commands,"subtree_receipts":index,
            "leaf_results":[{"task":"child","agent":"builder","status":"failed","result":"compilation failed"}]});
        let text = render("status", &task, false);
        for visible in [
            "Outcome: attempts exhausted",
            "child builder [failed] compilation failed",
            "verification: failed",
            "Observed verify [failed] check 5",
            "2 earlier command receipts",
            "t0 builder: verify [failed] make test",
            "1 more (use status --json)",
        ] {
            assert!(text.contains(visible), "{text}");
        }
        assert!(!text.contains("Model report:") && !text.contains(&"x".repeat(161)));
        assert_eq!(serde_json::from_str::<Value>(&render("status", &task, true)).unwrap(), task);
    }

    #[test]
    fn machine_output_round_trips_even_for_terminal_control_characters() {
        let task = json!({"id":"t", "status":"unknown", "result":"a\u{1b}\u{9b}b", "extra":[2]});
        let rendered = sanitize_terminal(&render("status", &task, true));
        assert!(!rendered.chars().any(char::is_control));
        assert_eq!(serde_json::from_str::<Value>(&rendered).unwrap(), task);
        for state in ["done", "failed", "cancelled", "unknown"] {
            assert!(is_terminal_str(state));
        }
        assert!(!is_terminal_str("approval"));
    }
}
