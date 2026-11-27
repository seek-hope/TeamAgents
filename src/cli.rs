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

  teamagents [chat] [--review] [--staged] [--plain] Submit prompts and receive final reports (TUI on TTY)
  teamagents tui [--review] [--staged]   Launch the interactive terminal interface
  teamagents exec [OPTIONS] TEXT         Submit a task (use - or pipe stdin for text)
  teamagents continue TASK_ID [OPTIONS] TEXT  Continue a settled leaf or summarizer as the next subtree
  teamagents whiteboard ROOT_ID [set|append|resolve|delete KEY [TEXT]]  Inspect or update a tree whiteboard
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
  --detach             Return the submission receipt without waiting (exec and continue)
  --timeout SECONDS    Limit the wait after submission (exec and continue wait indefinitely)
  --review             Require approval for each shell command and edit requested by
                       the model on this round or its children (exec, continue, chat, and tui)
  --staged             Use copy-on-exit workspace staging instead of direct workspace-write bwrap
  --check COMMAND      Run this exact trusted command at proposed completion (exec and continue).
                       A failure returns diagnostics for repair, up to 10 check attempts.

By default, agents work unattended in a direct workspace-write bwrap sandbox (protecting .git/admin dirs
and credential stores) with network access; pass --staged for isolated copy-on-exit staging. Leaves
collaborate via the per-tree Collaborative Whiteboard (whiteboard ROOT_ID / TUI 5 Board).
Leaving or timing out keeps work running. Use status to reconnect and tasks to find pending approvals.
When a root needs clarification, exec prints its question and exits. Use /model in chat to select model
and reasoning effort, answer ROOT_ID TEXT to clarify, or continue TASK_ID TEXT on a sole leaf/summarizer.";

const CHAT_HELP: &str = "Opening chat starts a fresh session. In a settled session, plain input continues
the sole leaf or summarizer node as the next subtree root (unless a question waits).
/new [TEXT]   /to NAME   /members   /tasks   /status TASK_ID   /cancel TASK_ID
/wb [KEY TEXT|append KEY TEXT|resolve KEY|delete KEY]   /staged [on|off]
/answer ROOT_ID TEXT   /approve APPROVAL_ID   /deny APPROVAL_ID   /help   /quit
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
    staged: bool,
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
    Chat { agent: String, review: bool, staged: bool, plain: bool },
    Tui { agent: String, review: bool, staged: bool },
    Exec(ExecOptions),
    Continue { id: String, opt: ExecOptions },
    Whiteboard { id: String, write: Option<(String, String, Option<String>, Option<String>)> },
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

fn parse_wb_kind_key(raw: &str) -> (Option<String>, String) {
    if let Some((k, rest)) = raw.split_once(':') {
        if matches!(k, "note" | "contract" | "claim" | "question" | "decision") && !rest.is_empty() {
            return (Some(k.to_owned()), rest.to_owned());
        }
    }
    (None, raw.to_owned())
}

fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Parsed> {
    let mut session = None;
    let mut workspace = None;
    let mut agent = String::from("default");
    let mut json = false;
    let mut detach = false;
    let mut timeout = None;
    let mut review = false;
    let mut staged = false;
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
            "--staged" => staged = true,
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

    if !matches!(cmd_name.as_str(), "exec" | "continue") && (check.is_some() || detach || timeout.is_some()) {
        return Err("--check, --timeout and --detach apply only to exec and continue".into());
    }
    if review && !matches!(cmd_name.as_str(), "chat" | "exec" | "continue" | "tui") {
        return Err("--review applies only to exec, continue, chat, and tui".into());
    }
    if staged && !matches!(cmd_name.as_str(), "chat" | "exec" | "continue" | "tui") {
        return Err("--staged applies only to exec, continue, chat, and tui".into());
    }
    if cmd_name == "continue" && agent != "default" {
        return Err("--agent applies only when starting a new session".into());
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
        "exec" => Command::Exec(ExecOptions { agent, review, staged, check, detach, timeout, args: pos_args }),
        "continue" => {
            if pos_args.len() < 2 || pos_args[0].trim().is_empty() {
                return Err("continue requires TASK_ID and a prompt (or - for stdin)".into());
            }
            let mut iter = pos_args.into_iter();
            let id = iter.next().unwrap();
            let args = iter.collect();
            Command::Continue { id, opt: ExecOptions { agent, review, staged, check, detach, timeout, args } }
        }
        "whiteboard" | "wb" => {
            let Some(id) = pos_args.first().filter(|s| !s.trim().is_empty()).cloned() else {
                return Err("whiteboard requires ROOT_ID [set|append|resolve|delete KEY [TEXT]]".into());
            };
            let write = match pos_args.get(1).map(String::as_str) {
                None => None,
                Some(mode @ ("set" | "append" | "resolve" | "delete")) => {
                    let Some(raw_key) = pos_args.get(2).filter(|s| !s.trim().is_empty()) else {
                        return Err(format!("whiteboard {mode} requires KEY"));
                    };
                    let (kind, key) = parse_wb_kind_key(raw_key);
                    let rest = if pos_args.len() > 3 { Some(pos_args[3..].join(" ")) } else { None };
                    if matches!(mode, "set" | "append") && rest.as_deref().is_none_or(|s| s.trim().is_empty()) {
                        return Err(format!("whiteboard {mode} requires TEXT"));
                    }
                    Some((key, mode.to_owned(), kind, rest))
                }
                _ => return Err("whiteboard mode must be set, append, resolve, or delete".into()),
            };
            Command::Whiteboard { id, write }
        }
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
                "chat" => Command::Chat { agent, review, staged, plain },
                "tui" => Command::Tui { agent, review, staged },
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
    if let Some(used) = task["turns_used"].as_u64() {
        if let Some(round) = task["round"].as_u64().filter(|&r| r > 1) {
            out.push_str(&format!(" | round {round}"));
        }
        if let Some(budget) = task["turn_budget"].as_u64() {
            out.push_str(&format!(" | root turns {used}/{budget}"));
        } else {
            out.push_str(&format!(" | turns {used}"));
        }
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
    if let Some(wb) = task["whiteboard"].as_array().filter(|rows| !rows.is_empty()) {
        let seq = task["whiteboard_seq"].as_u64().unwrap_or(0);
        out.push_str(&format!("\n  whiteboard: {} entry/entries (seq {seq})", wb.len()));
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
        "whiteboard" => {
            let root = value["root"].as_str().unwrap_or("?");
            let seq = value["seq"].as_u64().unwrap_or(0);
            let Some(entries) = value["entries"].as_array() else {
                return value.to_string();
            };
            if entries.is_empty() {
                return format!("Whiteboard {root} (seq {seq}): empty.");
            }
            let mut out = format!("Whiteboard {root} (seq {seq}, {} entries):", entries.len());
            for e in entries {
                let kind = e["kind"].as_str().unwrap_or("note");
                let key = e["key"].as_str().unwrap_or("?");
                let ver = e["version"].as_u64().unwrap_or(1);
                let eseq = e["seq"].as_u64().unwrap_or(0);
                let author = e["author_agent"].as_str().unwrap_or("?");
                let status = e["status"].as_str().unwrap_or("active");
                let body = preview(e["content"].as_str().unwrap_or(""), 200);
                out.push_str(&format!("\n  [{kind}] {key} (v{ver}, seq {eseq}, @{author}, {status}): {body}"));
            }
            out
        }
        "wb_write" => format!(
            "Whiteboard {} {} (seq {})",
            display_value(&value["mode"]),
            display_value(&value["key"]),
            value["seq"].as_u64().unwrap_or(0)
        ),
        "submit" => format!("Submitted {}", display_value(&value["id"])),
        "continue" => format!("Continued {} via node {}", display_value(&value["id"]), display_value(&value["node"])),
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
    let receipt =
        client.submit_with_options(&opt.agent, &prompt(&opt.args)?, opt.review, opt.check.as_deref(), opt.staged)?;
    wait_for_root(client, receipt, "submit", &opt, machine)
}

fn continue_cmd(client: Client, id: &str, opt: ExecOptions, machine: bool) -> Result<()> {
    let receipt =
        client.continue_with_options(id, &prompt(&opt.args)?, opt.review, opt.check.as_deref(), opt.staged)?;
    wait_for_root(client, receipt, "continue", &opt, machine)
}

fn wait_for_root(client: Client, receipt: Value, op: &str, opt: &ExecOptions, machine: bool) -> Result<()> {
    if opt.detach {
        return output(&render(op, &receipt, machine));
    }
    let id = field(&receipt, "id")?.to_owned();
    if !machine {
        let banner = if op == "continue" {
            let node = receipt["node"].as_str().unwrap_or(id.as_str());
            format!("Continued {id} via node {node}. Reconnect with status {id}.")
        } else {
            format!("Submitted {id}. Reconnect with status {id}.")
        };
        eprintln!("{}", sanitize_terminal(&banner));
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

fn chat_plain(client: &Client, initial_agent: &str, review: bool, initial_staged: bool, machine: bool) -> Result<()> {
    let mut agent = initial_agent.to_owned();
    let mut staged = initial_staged;
    let mut active_root: Option<String> = None;
    if !machine && io::stdin().is_terminal() {
        let mode = if review { "individual action review" } else { "unattended" };
        let sb = if staged { "staged" } else { "direct" };
        output(&format!("New session with {agent} ({mode}, {sb}). Enter a task, or /help for commands."))?;
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
    let submit = |agent: &str, text: &str, staged: bool| -> Result<String> {
        let receipt = client.submit_with_options(agent, text, review, None, staged)?;
        let id = field(&receipt, "id")?.to_owned();
        if machine || review {
            output(&render("submit", &receipt, machine))?;
        }
        Ok(id)
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
                    ("new", None, None) => {
                        active_root = None;
                        output(&if machine {
                            json!({"new_session":true}).to_string()
                        } else {
                            "Started a new session. Enter a task, or /help for commands.".into()
                        })
                    }
                    ("staged", None, None) => {
                        staged = !staged;
                        output(&if machine {
                            json!({"staged": staged}).to_string()
                        } else {
                            format!("Sandbox mode: {}.", if staged { "staged" } else { "direct" })
                        })
                    }
                    ("staged", Some(mode @ ("on" | "off")), None) => {
                        staged = mode == "on";
                        output(&if machine {
                            json!({"staged": staged}).to_string()
                        } else {
                            format!("Sandbox mode set to {}.", if staged { "staged" } else { "direct" })
                        })
                    }
                    ("wb", _, _) => {
                        let Some(root_id) = active_root.as_deref() else {
                            return Err("start or select a session first before using /wb".into());
                        };
                        if args.is_empty() {
                            command(client, "whiteboard", Some(root_id), machine)
                        } else {
                            let mut wb_parts = args.split_whitespace();
                            let first = wb_parts.next().unwrap();
                            let value = match first {
                                "resolve" => {
                                    let Some(raw_key) = wb_parts.next() else {
                                        return Err("/wb resolve requires KEY".into());
                                    };
                                    let (kind, key) = parse_wb_kind_key(raw_key);
                                    let rest = args
                                        .split_once("resolve")
                                        .unwrap()
                                        .1
                                        .trim_start()
                                        .split_once(raw_key)
                                        .map(|(_, t)| t.trim())
                                        .filter(|t| !t.is_empty());
                                    client.wb_write(root_id, &key, "resolve", kind.as_deref(), rest)?
                                }
                                "delete" => {
                                    let Some(raw_key) = wb_parts.next() else {
                                        return Err("/wb delete requires KEY".into());
                                    };
                                    let (_, key) = parse_wb_kind_key(raw_key);
                                    client.wb_write(root_id, &key, "delete", None, None)?
                                }
                                "append" => {
                                    let Some(raw_key) = wb_parts.next() else {
                                        return Err("/wb append requires KEY TEXT".into());
                                    };
                                    let (kind, key) = parse_wb_kind_key(raw_key);
                                    let body = args
                                        .split_once("append")
                                        .unwrap()
                                        .1
                                        .trim_start()
                                        .split_once(raw_key)
                                        .map(|(_, t)| t.trim())
                                        .unwrap_or("");
                                    client.wb_write(root_id, &key, "append", kind.as_deref(), Some(body))?
                                }
                                raw_key => {
                                    let (kind, key) = parse_wb_kind_key(raw_key);
                                    let body = args.split_once(raw_key).map(|(_, t)| t.trim()).unwrap_or("");
                                    client.wb_write(root_id, &key, "set", kind.as_deref(), Some(body))?
                                }
                            };
                            output(&render("wb_write", &value, machine))
                        }
                    }
                    ("new", Some(_), _) => match submit(&agent, args, staged) {
                        Ok(id) => {
                            active_root = Some(id);
                            Ok(())
                        }
                        Err(e) => Err(e),
                    },
                    ("tasks" | "members", None, None) | ("status" | "cancel" | "approve" | "deny", Some(_), None) => {
                        command(client, op, arg, machine)
                    }
                    _ => Err(format!("invalid /{op} command; see /help")),
                }
            } else {
                let target = tracker.lock().unwrap().reply_target(&agent);
                match target {
                    Ok(Some(id)) => answer(&id, line),
                    Ok(None) => {
                        if let Some(root_id) = active_root.as_deref() {
                            match client.continue_with_options(root_id, line, review, None, staged) {
                                Ok(receipt) => {
                                    if machine || review {
                                        output(&render("continue", &receipt, machine))
                                    } else {
                                        Ok(())
                                    }
                                }
                                Err(e) => Err(e),
                            }
                        } else {
                            match submit(&agent, line, staged) {
                                Ok(id) => {
                                    active_root = Some(id);
                                    Ok(())
                                }
                                Err(e) => Err(e),
                            }
                        }
                    }
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
        Command::Tui { agent, review, staged } => {
            if !interactive_tty {
                return Err(
                    "tui requires an interactive terminal; use `teamagents chat --plain` or `teamagents exec`".into()
                );
            }
            crate::tui::run(client, agent, review, staged)
        }
        Command::Chat { agent, review, staged, plain } => {
            if !global.json && !plain && interactive_tty {
                crate::tui::run(client, agent, review, staged)
            } else {
                chat_plain(&client, &agent, review, staged, global.json)
            }
        }
        Command::Exec(opt) => exec(client, opt, global.json),
        Command::Continue { id, opt } => continue_cmd(client, &id, opt, global.json),
        Command::Whiteboard { id, write: None } => command(&client, "whiteboard", Some(&id), global.json),
        Command::Whiteboard { id, write: Some((key, mode, kind, content)) } => {
            let value = client.wb_write(&id, &key, &mode, kind.as_deref(), content.as_deref())?;
            output(&render("wb_write", &value, global.json))
        }
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
            options(&["--review", "--staged", "tui", "--agent", "builder"]).unwrap().command,
            Command::Tui { ref agent, review: true, staged: true } if agent == "builder"
        ));
        assert!(matches!(
            options(&["exec", "--staged", "run"]).unwrap().command,
            Command::Exec(ref opt) if opt.staged
        ));
        assert!(matches!(
            options(&["whiteboard", "r1", "set", "contract:api", "v1 schema"]).unwrap().command,
            Command::Whiteboard { ref id, write: Some((ref key, ref mode, Some(ref kind), Some(ref text))) }
                if id == "r1" && key == "api" && mode == "set" && kind == "contract" && text == "v1 schema"
        ));
        assert!(matches!(
            options(&["delete", "r1"]).unwrap().command,
            Command::Delete { ref id } if id == "r1"
        ));
        assert!(matches!(
            options(&["continue", "n1", "--review", "--check", "cargo test", "--detach", "follow", "up"]).unwrap().command,
            Command::Continue { ref id, ref opt }
                if id == "n1" && opt.review && opt.detach && opt.check.as_deref() == Some("cargo test")
                && prompt(&opt.args).unwrap() == "follow up"
        ));
        for args in [
            &["deny"][..],
            &["delete"],
            &["delete", "r1", "extra"],
            &["continue"],
            &["continue", "n1"],
            &["continue", " ", "follow up"],
            &["continue", "--agent", "worker", "n1", "follow up"],
            &["answer"],
            &["answer", "t1"],
            &["answer", "t1", " "],
            &["status", "a", "b"],
            &["exec", "--timeout", "-1"],
            &["--check", "x", "tasks"],
            &["tasks", "--review"],
            &["tasks", "--staged"],
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
