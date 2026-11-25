use crate::{daemon, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(500);
const HELP: &str = "TeamAgents — persistent agent sessions

  teamagents [chat] [--review]           Submit prompts and receive final reports
  teamagents exec [OPTIONS] TEXT         Submit a task (use - or pipe stdin for text)
  teamagents members | tasks             List session members or tasks
  teamagents status TASK_ID              Inspect progress, result and pending action
  teamagents answer ROOT_ID TEXT          Answer a root clarification and resume work
  teamagents cancel TASK_ID              Request cancellation
  teamagents approve | deny APPROVAL_ID  Decide one pending action, not a whole task
  teamagents auth login | status | logout  Manage ChatGPT subscription sign-in
  teamagents stop                        Stop an idle daemon; keep the session
  teamagents help | version

Options may appear before or after the command. Use -- before literal arguments.
  --session PATH       Session directory (default: per workspace under XDG state)
  --workspace PATH     Workspace (default: current directory)
  --agent NAME         Recipient for a new task or chat (default: default)
  --json               Print machine-readable JSON
  --detach             Return the submission receipt without waiting (exec)
  --timeout SECONDS    Limit the wait after submission (exec waits indefinitely by default)
  --review             Require approval for each shell command and edit requested by
                       the model on this root or its children (exec and chat)
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

struct Options {
    session: Option<PathBuf>,
    workspace: Option<PathBuf>,
    agent: String,
    json: bool,
    detach: bool,
    timeout: Option<Duration>,
    review: bool,
    check: Option<String>,
    command: String,
    args: Vec<String>,
}

fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Options> {
    let mut opt = Options {
        session: None,
        workspace: None,
        agent: "default".into(),
        json: false,
        detach: false,
        timeout: None,
        review: false,
        check: None,
        command: String::new(),
        args: Vec::new(),
    };
    let mut args = args.into_iter();
    let mut literal = false;
    let mut info = None;
    while let Some(arg) = args.next() {
        let word = arg.to_str().ok_or("command and prompt arguments must be UTF-8")?;
        if literal {
            if opt.command.is_empty() {
                opt.command = word.into();
            } else {
                opt.args.push(word.into());
            }
            continue;
        }
        let mut value = || args.next().filter(|v| !v.is_empty()).ok_or_else(|| format!("{word} requires a value"));
        let text = |v: OsString| v.into_string().map_err(|_| format!("{word} requires UTF-8 text"));
        match word {
            "--" => literal = true,
            "--session" => opt.session = Some(value()?.into()),
            "--workspace" => opt.workspace = Some(value()?.into()),
            "--agent" => opt.agent = text(value()?)?,
            "--check" => opt.check = Some(text(value()?)?),
            "--timeout" => {
                let seconds = text(value()?)?.parse().map_err(|_| "--timeout requires nonnegative whole seconds")?;
                opt.timeout = Some(Duration::from_secs(seconds));
            }
            "--json" => opt.json = true,
            "--review" => opt.review = true,
            "--detach" => opt.detach = true,
            "--help" | "-h" => info = Some("help"),
            "--version" | "-V" => info = Some("version"),
            _ if word.starts_with('-') && word != "-" => {
                return Err(format!("unknown option: {word} (use -- for literal text)"))
            }
            _ if opt.command.is_empty() => opt.command = word.into(),
            _ => opt.args.push(word.into()),
        }
    }
    if let Some(info) = info {
        opt.command = info.into();
        opt.args.clear();
    }
    if opt.command.is_empty() {
        opt.command = "chat".into();
    }
    match opt.command.as_str() {
        "help" | "version" => return Ok(opt),
        "exec" => {}
        "auth" => {
            if opt.args.len() != 1 || !matches!(opt.args[0].as_str(), "login" | "status" | "logout") {
                return Err("use teamagents auth login, status or logout".into());
            }
            if opt.json || opt.session.is_some() || opt.workspace.is_some() || opt.agent != "default" {
                return Err(
                    "auth manages the local user account and does not take session, workspace, agent or JSON options"
                        .into(),
                );
            }
        }
        "answer" => {
            if opt.args.len() < 2 || opt.args[0].trim().is_empty() || opt.args[1..].iter().all(|s| s.trim().is_empty())
            {
                return Err("answer requires ROOT_ID and nonempty TEXT".into());
            }
        }
        "cancel" | "status" | "approve" | "deny" => {
            if opt.args.len() != 1 || opt.args[0].trim().is_empty() {
                return Err(format!("{} requires exactly one ID", opt.command));
            }
        }
        "chat" | "daemon" | "members" | "tasks" | "stop" => {
            if !opt.args.is_empty() {
                return Err(format!("{} does not take arguments", opt.command));
            }
        }
        cmd => return Err(format!("unknown command: {cmd} (see teamagents help)")),
    }
    if opt.command != "exec" && (opt.check.is_some() || opt.detach || opt.timeout.is_some()) {
        return Err("--check, --timeout and --detach apply only to exec".into());
    }
    if opt.review && !matches!(opt.command.as_str(), "chat" | "exec") {
        return Err("--review applies only to exec and chat".into());
    }
    if opt.agent.trim().is_empty() {
        return Err("--agent requires a name".into());
    }
    if opt.check.as_deref().is_some_and(|s| s.trim().is_empty()) {
        return Err("--check requires a command".into());
    }
    Ok(opt)
}

#[derive(Clone)]
struct Client {
    session: PathBuf,
    workspace: PathBuf,
}

impl Client {
    fn new(opt: &Options) -> Result<Self> {
        let cwd = env::current_dir().map_err(|e| e.to_string())?;
        let workspace = cwd.join(opt.workspace.as_deref().unwrap_or(Path::new(".")));
        let workspace = workspace.canonicalize().map_err(|e| format!("workspace {}: {e}", workspace.display()))?;
        let session = if let Some(path) = &opt.session {
            cwd.join(path)
        } else {
            let state = env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .or_else(|| env::var_os("HOME").filter(|p| !p.is_empty()).map(|p| cwd.join(p).join(".local/state")))
                .ok_or("set XDG_STATE_HOME, HOME, or --session")?;
            state.join("teamagents-swarm/sessions").join(workspace_key(&workspace))
        };
        Ok(Self { session, workspace })
    }

    fn call(&self, body: Value) -> Result<Value> {
        daemon::request(&self.session, &self.workspace, &body)
    }
    fn tasks(&self) -> Result<Vec<Value>> {
        match self.call(json!({"op":"tasks"}))? {
            Value::Array(rows) => Ok(rows),
            _ => Err("invalid tasks response".into()),
        }
    }
    fn answer(&self, id: &str, text: &str, machine: bool) -> Result<()> {
        let value = self.call(json!({"op":"answer", "id":id, "text":text}))?;
        output(&render("answer", &value, machine))
    }
    fn submit(&self, agent: &str, text: &str, review: bool, check: Option<&str>) -> Result<Value> {
        if text.trim().is_empty() {
            return Err("a nonempty task prompt is required".into());
        }
        let mut body = json!({"op":"submit", "agent":agent, "text":text, "review":review});
        if let Some(command) = check {
            body["check"] = json!(command);
        }
        let receipt = self.call(body)?;
        field(&receipt, "id")?;
        Ok(receipt)
    }
}

// FNV-1a is deliberately fixed so upgrading Rust does not change the default session.
fn workspace_key(path: &Path) -> String {
    let hash = path
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100000001b3));
    format!("{hash:016x}")
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key].as_str().filter(|s| !s.is_empty()).ok_or_else(|| format!("invalid daemon response: missing {key}"))
}

fn terminal(status: &str) -> bool {
    matches!(status, "done" | "failed" | "cancelled" | "unknown")
}

fn display_value(value: &Value) -> String {
    value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string())
}

// Keep final feedback readable even for a command with pages of output. JSON keeps the full receipt.
fn preview(text: &str) -> String {
    let mut chars = text.trim().chars().map(|c| if c.is_whitespace() { ' ' } else { c });
    let mut short: String = chars.by_ref().take(160).collect();
    if chars.next().is_some() {
        short.push('…');
    }
    short
}

fn outcome(receipt: &Value) -> &'static str {
    match receipt["ok"].as_bool() {
        Some(true) => "ok",
        Some(false) => "failed",
        None => "unknown",
    }
}

fn append_evidence(out: &mut String, task: &Value) {
    if let Some(receipts) = task["evidence"].as_array() {
        out.push_str("\n  Model reports alone do not establish that tests passed.");
        let commands: Vec<_> = receipts.iter().filter(|r| r["action"]["command"].is_string()).collect();
        if commands.is_empty() {
            out.push_str("\n  No completed command receipts on this task.");
        }
        for r in commands.iter().rev().take(4).rev() {
            let command = preview(r["action"]["command"].as_str().unwrap());
            let result = preview(r["result"].as_str().unwrap_or(""));
            out.push_str(&format!(
                "\n  Observed {} [{}] {command} — {result}",
                r["action"]["op"].as_str().unwrap_or("command"),
                outcome(r)
            ));
        }
        if commands.len() > 4 {
            out.push_str(&format!("\n  {} earlier command receipts omitted (use status --json).", commands.len() - 4));
        }
    }
    if let Some(index) = task["subtree_receipts"].as_array().filter(|i| !i.is_empty()) {
        out.push_str("\n  Subtree receipts (inspect a task ID for details):");
        for r in index.iter().take(8) {
            let label = preview(r["label"].as_str().unwrap_or(""));
            out.push_str(&format!(
                "\n    {} {}: {} [{}]",
                r["task"].as_str().unwrap_or("?"),
                r["agent"].as_str().unwrap_or("?"),
                r["op"].as_str().unwrap_or("?"),
                outcome(r)
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
            out.push_str(&format!("\n  Outcome: {}", preview(result)));
        }
    }
    if let Some(error) = task["stage_error"].as_str() {
        out.push_str(&format!("\n  Worker error: {}", preview(error)));
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
            let result = preview(leaf["result"].as_str().unwrap_or(""));
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

fn terminal_text(text: &str) -> String {
    // Neutralize terminal controls without changing the contents of JSON strings.
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() && !matches!(c, '\n' | '\t') {
            out.push_str(&format!("\\u{:04x}", u32::from(c)));
        } else {
            out.push(c);
        }
    }
    out
}

fn output(line: &str) -> Result<()> {
    let mut out = io::stdout().lock();
    writeln!(out, "{}", terminal_text(line)).and_then(|_| out.flush()).map_err(|e| e.to_string())
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

fn exec(client: Client, opt: &Options) -> Result<()> {
    let receipt = client.submit(&opt.agent, &prompt(&opt.args)?, opt.review, opt.check.as_deref())?;
    if opt.detach {
        return output(&render("submit", &receipt, opt.json));
    }
    let id = field(&receipt, "id")?.to_owned();
    if !opt.json {
        eprintln!("{}", terminal_text(&format!("Submitted {id}. Reconnect with status {id}.")));
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
            status.as_ref().map_or(true, |v| v["status"].as_str().is_none_or(|s| terminal(s) || s == "input"));
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
            output(&render("status", &task, opt.json))?;
            return Err(format!("input required; resume with teamagents answer {id} TEXT"));
        }
        if terminal(status) {
            output(&render("status", &task, opt.json))?;
            return if status == "done" { Ok(()) } else { Err(format!("task {id} {status}")) };
        }
        if !opt.json && status == "approval" && last != task {
            eprintln!("{}", terminal_text(&task_text(&task)));
        }
        last = task;
    }
    output(&render(if last.get("status").is_some() { "status" } else { "submit" }, &last, opt.json))?;
    Err(format!("timed out waiting for {id}; reconnect with status {id}"))
}

// Track the whole session, not just locally submitted roots. Child proposals and
// changes made from another terminal must reach the chat without more keyboard input.
#[derive(Default)]
struct Watch {
    // Deduplicate notices without retaining full task records or tool output.
    seen: HashMap<String, (String, Option<String>)>,
    questions: HashMap<String, String>, // announced root ID -> recipient
    initialized: bool,
}

impl Watch {
    fn update(&mut self, tasks: Vec<Value>, mut show: impl FnMut(&Value) -> Result<()>) -> Result<()> {
        for task in tasks {
            let id = field(&task, "id")?.to_owned();
            let status = field(&task, "status")?;
            let waiting = status == "input" && task["parent"].is_null();
            let detail = if waiting { &task["question"] } else { &task["approval_id"] };
            let key = (status.to_owned(), detail.as_str().map(str::to_owned));
            if self.seen.get(&id) == Some(&key) {
                continue;
            }
            if waiting || status == "approval" || (self.initialized && task["parent"].is_null() && terminal(status)) {
                show(&task)?;
            }
            if waiting {
                self.questions.insert(id.clone(), field(&task, "agent")?.to_owned());
            }
            self.seen.insert(id, key);
        }
        self.initialized = true;
        Ok(())
    }

    fn reply_to(&mut self, agent: &str) -> Result<Option<String>> {
        // A different client may have answered or cancelled while this user typed.
        // Warn once before treating a line as a new task or redirecting it to the remaining root.
        let announced = self.questions.len();
        self.questions.retain(|id, _| self.seen.get(id).is_some_and(|(status, _)| status == "input"));
        if self.questions.len() != announced {
            return Err(
                "A waiting question closed in another client. Nothing was sent. Use /answer ROOT_ID TEXT or /new TEXT."
                    .into(),
            );
        }
        let mut questions = self.questions.iter();
        match questions.next() {
            None => Ok(None),
            Some((id, recipient)) if recipient == agent && questions.next().is_none() => Ok(Some(id.clone())),
            _ => Err("Specify /answer ROOT_ID TEXT for a waiting question or /new TEXT for a separate task.".into()),
        }
    }
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

fn chat(client: &Client, opt: &Options) -> Result<()> {
    let mut agent = opt.agent.clone();
    if !opt.json && io::stdin().is_terminal() {
        let mode = if opt.review { "individual action review" } else { "unattended" };
        output(&format!("Chatting with {agent} ({mode}). Enter a task, or /help for commands."))?;
    }
    let watch = Mutex::new(Watch::default());
    let show = |task: &Value| {
        if terminal(field(task, "status")?) {
            // The list omits receipts. Fetch the final status only when the root finishes.
            command(client, "status", Some(field(task, "id")?), opt.json)
        } else {
            output(&render("status", task, opt.json))
        }
    };
    watch.lock().unwrap().update(client.tasks()?, show)?;
    let answer = |id: &str, text: &str| {
        client.answer(id, text, opt.json)?;
        let mut watch = watch.lock().unwrap();
        watch.questions.remove(id);
        watch.seen.remove(id);
        Ok(())
    };
    let submit = |agent: &str, text: &str| {
        let receipt = client.submit(agent, text, opt.review, None)?;
        if opt.json || opt.review {
            output(&render("submit", &receipt, opt.json))
        } else {
            Ok(())
        }
    };
    let (stop, stopped) = mpsc::channel::<()>();
    thread::scope(|scope| {
        let watch = &watch;
        scope.spawn(move || {
            let mut previous_error = None;
            while stopped.recv_timeout(POLL) == Err(mpsc::RecvTimeoutError::Timeout) {
                let error = client.tasks().and_then(|tasks| watch.lock().unwrap().update(tasks, show)).err();
                if error != previous_error {
                    if let Some(message) = &error {
                        eprintln!("task updates: {}", terminal_text(message));
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
                    ("help", None, None) => output(&render("help", &json!(CHAT_HELP), opt.json)),
                    ("model", _, _) => model_command(client, args, opt.json),
                    ("to", Some(name), None) => {
                        agent = name.into();
                        output(&if opt.json {
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
                        command(client, op, arg, opt.json)
                    }
                    _ => Err(format!("invalid /{op} command; see /help")),
                }
            } else {
                let target = watch.lock().unwrap().reply_to(&agent);
                match target {
                    Ok(Some(id)) => answer(&id, line),
                    Ok(None) => submit(&agent, line),
                    Err(e) => Err(e),
                }
            };
            if let Err(e) = result {
                eprintln!("teamagents: {}", terminal_text(&e));
            }
        }
        Ok(())
    })
}

pub fn run() -> Result<()> {
    run_cli().map_err(|e| terminal_text(&e))
}

fn run_cli() -> Result<()> {
    let opt = parse(env::args_os().skip(1))?;
    match opt.command.as_str() {
        "help" => return output(&render("help", &json!(HELP), opt.json)),
        "version" => return output(&render("version", &json!(env!("CARGO_PKG_VERSION")), opt.json)),
        "auth" => {
            return match opt.args[0].as_str() {
                "login" => crate::oauth::login(),
                "status" => crate::oauth::status(),
                "logout" => crate::oauth::logout(),
                _ => unreachable!("validated auth command"),
            };
        }
        _ => {}
    }
    let client = Client::new(&opt)?;
    match opt.command.as_str() {
        "daemon" => daemon::serve(&client.session, &client.workspace),
        "chat" => chat(&client, &opt),
        "exec" => exec(client, &opt),
        "answer" => client.answer(&opt.args[0], &opt.args[1..].join(" "), opt.json),
        op => command(&client, op, opt.args.first().map(String::as_str), opt.json),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(args: &[&str]) -> Result<Options> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn options_and_literal_prompt_do_not_depend_on_flag_position() {
        let opt = options(&[
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
        assert_eq!(opt.session.as_deref(), Some(Path::new("s")));
        assert_eq!(opt.workspace.as_deref(), Some(Path::new("w")));
        assert!(opt.json && opt.detach);
        assert_eq!(opt.agent, "worker");
        assert_eq!(opt.timeout, Some(Duration::from_secs(2)));
        assert_eq!(opt.check.as_deref(), Some("cargo test"));
        assert_eq!(prompt(&opt.args).unwrap(), "--help literal");
        assert_eq!(options(&[]).unwrap().command, "chat");
        for command in ["chat", "exec"] {
            let opt = options(&[command]).unwrap();
            assert!(!opt.review && opt.timeout.is_none());
            assert!(options(&["--review", command]).unwrap().review);
        }
        for args in [
            &["deny"][..],
            &["answer"],
            &["answer", "t1"],
            &["answer", "t1", " "],
            &["status", "a", "b"],
            &["exec", "--timeout", "-1"],
            &["--check", "x", "tasks"],
            &["tasks", "--review"],
            &["chat", "--timeout", "1"],
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
        let mut watch = Watch::default();
        watch.update(vec![finished.clone(), child.clone()], &mut show).unwrap();
        watch.update(vec![finished, child], &mut show).unwrap();
        watch
            .update(vec![json!({"id":"t2", "parent":"t1", "status":"done", "result":"child report"})], &mut show)
            .unwrap();
        watch.update(vec![json!({"id":"t1", "status":"done", "result":"all tests passed"})], &mut show).unwrap();
        assert_eq!(seen.len(), 2);
        for s in ["child of t1", "approval_id: a1", "\"command\":\"cargo test\""] {
            assert!(seen[0].contains(s));
        }
        assert!(seen[1].contains("all tests passed"));
    }

    #[test]
    fn natural_reply_never_redirects_a_stale_answer() {
        let mut watch = Watch::default();
        let first = json!({"id":"first","agent":"a","status":"input","question":"Which target?"});
        let second = json!({"id":"second","agent":"a","status":"input","question":"Which version?"});
        watch.update(vec![first.clone(), second], |_| Ok(())).unwrap();
        assert!(watch.reply_to("a").is_err());
        watch.update(vec![first, json!({"id":"second","agent":"a","status":"queued"})], |_| Ok(())).unwrap();
        assert!(watch.reply_to("a").unwrap_err().contains("Nothing was sent"));
        assert!(watch.reply_to("different agent").is_err());
        assert_eq!(watch.reply_to("a").unwrap().as_deref(), Some("first"));
        watch.update(vec![json!({"id":"first","agent":"a","status":"cancelled"})], |_| Ok(())).unwrap();
        assert!(watch.reply_to("a").is_err());
        assert!(watch.reply_to("a").unwrap().is_none());
    }

    #[test]
    fn human_receipts_are_bounded_without_hiding_failure_or_changing_json() {
        let commands: Vec<_> = (0..6).map(|i| json!({"action":{"op":"verify","command":format!("check {i}")}, "ok":false,"result":"x".repeat(1000)})).collect();
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
        let rendered = terminal_text(&render("status", &task, true));
        assert!(!rendered.chars().any(char::is_control));
        assert_eq!(serde_json::from_str::<Value>(&rendered).unwrap(), task);
        for state in ["done", "failed", "cancelled", "unknown"] {
            assert!(terminal(state));
        }
        assert!(!terminal("approval"));
    }
}
