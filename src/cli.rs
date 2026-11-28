use crate::{
    daemon,
    mcp::{self, McpServerSpec},
    model::{self, CustomProviderSpec, ProviderInfo, ToolChoiceMode, WireProtocol},
    protocol::{
        field, is_terminal_str, outcome_label, preview, sanitize_terminal, Client, ProviderSummary, QuestionTracker,
    },
    sandbox::SandboxMode,
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

  teamagents [chat] [--review] [--staged|--sandbox MODE] [--plain] Submit prompts and receive reports (TUI on TTY)
  teamagents tui [--review] [--staged|--sandbox MODE]   Launch the interactive terminal interface
  teamagents exec [OPTIONS] TEXT         Submit a task (use - or pipe stdin for text)
  teamagents review [--uncommitted|--base REF|--commit SHA] [TEXT]  Run a read-only code review
  teamagents continue TASK_ID [OPTIONS] TEXT  Continue a settled leaf or summarizer as the next subtree
  teamagents resume [TASK_ID [TEXT]]     Resume the latest or named settled session
  teamagents steer TASK_ID TEXT          Send mid-turn guidance to an active session tree
  teamagents compact TASK_ID             Compact conversation history for a settled or idle node
  teamagents diff [--base REF|--commit SHA]  Show workspace git status and unified diff
  teamagents skills                      List discovered local SKILL.md skills
  teamagents mcp [list|add|remove|tools] Manage MCP servers in mcp.json (env-var refs only)
  teamagents whiteboard ROOT_ID [set|append|resolve|delete KEY [TEXT]]  Inspect or update a tree whiteboard
  teamagents members | tasks             List session members or tasks
  teamagents status TASK_ID              Inspect progress, result and pending action
  teamagents answer ROOT_ID TEXT          Answer a root clarification and resume work
  teamagents cancel TASK_ID              Request cancellation
  teamagents delete ROOT_ID              Delete a settled or idle session tree
  teamagents approve | deny APPROVAL_ID  Decide one pending action, not a whole task
  teamagents auth login | status | logout  Manage ChatGPT subscription sign-in
  teamagents provider [list|show|add|remove|default]  Manage custom model providers (env-var refs only)
  teamagents stop                        Stop an idle daemon; keep the session
  teamagents help | version

Options may appear before or after the command. Use -- before literal arguments.
  --session PATH       Session directory (default: per workspace under XDG state)
  --workspace PATH     Workspace (default: current directory)
  --agent NAME         Recipient for a new task or chat (default: default)
  --json               Print machine-readable JSON
  --jsonl              Stream newline-delimited live events during exec, continue, and review
  --plain              Use the line-oriented chat REPL even on a terminal (chat)
  --detach             Return the submission receipt without waiting (exec, continue, and review)
  --timeout SECONDS    Limit the wait after submission (exec, continue, and review wait indefinitely)
  --review             Require approval for each shell command and edit requested by
                       the model on this round or its children (exec, continue, review, chat, and tui)
  --sandbox MODE       Sandbox mode: direct (default), staged, or read-only
  --staged             Shorthand for --sandbox staged (copy-on-exit workspace staging)
  --check COMMAND      Run this exact trusted command at proposed completion (exec and continue).
                       A failure returns diagnostics for repair, up to 10 check attempts.

Review / diff / MCP options:
  --uncommitted        Review uncommitted workspace changes (default for `review`)
  --base REF           Diff or review against git base revision REF
  --commit SHA         Diff or review a specific commit SHA
  --command CMD        Stdio MCP server executable (for `mcp add NAME`)
  --arg VALUE          Repeatable stdio MCP server argument (for `mcp add NAME`)
  --env ENV_VAR        Repeatable host env-var name to forward to stdio MCP server (for `mcp add NAME`)
  --url URL            Streamable HTTP MCP endpoint URL (for `mcp add NAME`)
  --bearer-token-env VAR  Env var holding bearer token for HTTP MCP server (for `mcp add NAME`)

Provider options (with `provider add NAME`):
  --wire WIRE          Protocol wire: responses, chat_completions (or api), or anthropic
  --base-url URL       Endpoint URL (HTTPS required except localhost / 127.0.0.1 / [::1])
  --api-key-env VAR    Environment variable NAME holding the credential (required off loopback)
  --model ID           Default model ID for this provider
  --context-window N   Model context window in tokens
  --tool-choice MODE   Tool choice mode when tools attach: required (default) or auto
  --max-tokens N       Default max_tokens (Anthropic wire)
  --replay-reasoning   Replay reasoning_content on Chat Completions tool turns
  --default            Also make this provider the configured default

By default, agents work unattended in a direct workspace-write bwrap sandbox (protecting .git/admin dirs
and credential stores) with network access; pass --sandbox staged for isolated staging or --sandbox
read-only for analysis. Leaves coordinate via the Collaborative Whiteboard (whiteboard ROOT_ID / TUI 5 Board).
Leaving or timing out keeps work running. Use status or resume to reconnect and tasks to inspect approvals.
When a root needs clarification, exec prints its question and exits. Use `teamagents provider` to register
endpoints and `/model provider NAME` in chat/TUI to switch providers live without restarting.";

const CHAT_HELP: &str = "Opening chat starts a fresh session. In a settled session, plain input continues
the sole leaf or summarizer node as the next subtree root (or steers an active session in TUI).
/new [TEXT]   /plan TEXT   /review code [REF]   /to NAME   /members   /tasks   /status TASK_ID
/cancel TASK_ID   /steer TEXT   /compact   /diff [REF]   /skills   /mcp
/wb [KEY TEXT|append KEY TEXT|resolve KEY|delete KEY]   /sandbox [direct|staged|read-only]   /staged [on|off]
/answer ROOT_ID TEXT   /approve APPROVAL_ID   /deny APPROVAL_ID   /help   /quit
/model shows the selection for new roots. /model list queries visible provider models.
/model MODEL [EFFORT]   /model effort EFFORT   /model default
/model providers   /model provider NAME [MODEL [EFFORT]]
/provider [list|show NAME|use NAME|add NAME ...|remove NAME|default [NAME|auto]]";

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

#[derive(Clone, Debug, PartialEq, Eq)]
enum ProviderAction {
    List,
    Show(String),
    Add { name: String, spec: CustomProviderSpec, set_default: bool },
    Remove(String),
    DefaultGet,
    DefaultSet(Option<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ReviewTarget {
    Uncommitted,
    Base(String),
    Commit(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum McpAction {
    List,
    Add { name: String, spec: McpServerSpec },
    Remove(String),
    Tools(Option<String>),
}

struct ExecOptions {
    agent: String,
    review: bool,
    sandbox: SandboxMode,
    check: Option<String>,
    detach: bool,
    timeout: Option<Duration>,
    jsonl: bool,
    args: Vec<String>,
}

enum Command {
    Help,
    Version,
    Auth(AuthAction),
    Provider(ProviderAction),
    Daemon,
    Chat { agent: String, review: bool, sandbox: SandboxMode, plain: bool },
    Tui { agent: String, review: bool, sandbox: SandboxMode },
    Exec(ExecOptions),
    Continue { id: String, opt: ExecOptions },
    Review { target: ReviewTarget, opt: ExecOptions },
    Resume { id: Option<String>, opt: ExecOptions },
    Steer { id: String, text: String },
    Compact { id: String },
    Diff { base: Option<String>, commit: Option<String> },
    Skills,
    Mcp(McpAction),
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
    let mut sandbox_opt: Option<SandboxMode> = None;
    let mut jsonl = false;
    let mut plain = false;
    let mut check = None;
    let mut uncommitted = false;
    let mut base_ref: Option<String> = None;
    let mut commit_ref: Option<String> = None;
    let mut mcp_command: Option<String> = None;
    let mut mcp_args: Vec<String> = Vec::new();
    let mut mcp_env: Vec<String> = Vec::new();
    let mut mcp_url: Option<String> = None;
    let mut mcp_bearer_env: Option<String> = None;
    let mut wire: Option<String> = None;
    let mut base_url: Option<String> = None;
    let mut api_key_env: Option<String> = None;
    let mut model_opt: Option<String> = None;
    let mut context_window: Option<u64> = None;
    let mut tool_choice: Option<ToolChoiceMode> = None;
    let mut max_tokens: Option<u32> = None;
    let mut replay_reasoning = false;
    let mut make_default = false;
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
            "--sandbox" => sandbox_opt = Some(SandboxMode::parse(&text(value()?)?)?),
            "--uncommitted" => uncommitted = true,
            "--base" => base_ref = Some(text(value()?)?),
            "--commit" => commit_ref = Some(text(value()?)?),
            "--command" => mcp_command = Some(text(value()?)?),
            "--arg" => mcp_args.push(text(value()?)?),
            "--env" => mcp_env.push(text(value()?)?),
            "--url" => mcp_url = Some(text(value()?)?),
            "--bearer-token-env" => mcp_bearer_env = Some(text(value()?)?),
            "--wire" => wire = Some(WireProtocol::parse_custom(&text(value()?)?)?.as_str().to_owned()),
            "--base-url" => base_url = Some(text(value()?)?),
            "--api-key-env" => api_key_env = Some(text(value()?)?),
            "--model" => model_opt = Some(text(value()?)?),
            "--context-window" => {
                let n: u64 = text(value()?)?.parse().map_err(|_| "--context-window requires a positive integer")?;
                if n == 0 {
                    return Err("--context-window requires a positive integer".into());
                }
                context_window = Some(n);
            }
            "--tool-choice" => tool_choice = Some(ToolChoiceMode::parse(&text(value()?)?)?),
            "--max-tokens" => {
                let n: u32 = text(value()?)?.parse().map_err(|_| "--max-tokens requires a positive integer")?;
                if n == 0 {
                    return Err("--max-tokens requires a positive integer".into());
                }
                max_tokens = Some(n);
            }
            "--replay-reasoning" => replay_reasoning = true,
            "--default" => make_default = true,
            "--timeout" => {
                let seconds = text(value()?)?.parse().map_err(|_| "--timeout requires nonnegative whole seconds")?;
                timeout = Some(Duration::from_secs(seconds));
            }
            "--json" => json = true,
            "--jsonl" => jsonl = true,
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

    if staged && sandbox_opt.is_some_and(|m| m != SandboxMode::Staged) {
        return Err("--staged cannot be combined with a different --sandbox mode".into());
    }
    let has_provider_flags = wire.is_some()
        || base_url.is_some()
        || api_key_env.is_some()
        || model_opt.is_some()
        || context_window.is_some()
        || tool_choice.is_some()
        || max_tokens.is_some()
        || replay_reasoning
        || make_default;
    if cmd_name != "provider" && has_provider_flags {
        return Err(
            "--wire, --base-url, --api-key-env, --model, --context-window, --tool-choice, --max-tokens, --replay-reasoning and --default apply only to provider"
                .into(),
        );
    }
    let has_mcp_flags = mcp_command.is_some()
        || !mcp_args.is_empty()
        || !mcp_env.is_empty()
        || mcp_url.is_some()
        || mcp_bearer_env.is_some();
    if cmd_name != "mcp" && has_mcp_flags {
        return Err("--command, --arg, --env, --url and --bearer-token-env apply only to mcp".into());
    }
    if (uncommitted || base_ref.is_some() || commit_ref.is_some()) && !matches!(cmd_name.as_str(), "review" | "diff") {
        return Err("--uncommitted, --base and --commit apply only to review and diff".into());
    }
    if uncommitted && cmd_name == "diff" {
        return Err("--uncommitted applies only to review".into());
    }
    if usize::from(uncommitted) + usize::from(base_ref.is_some()) + usize::from(commit_ref.is_some()) > 1 {
        return Err("specify at most one of --uncommitted, --base, or --commit".into());
    }
    if !matches!(cmd_name.as_str(), "exec" | "continue" | "review" | "resume")
        && (check.is_some() || detach || timeout.is_some() || jsonl)
    {
        return Err("--check, --timeout, --detach and --jsonl apply only to exec, continue, review and resume".into());
    }
    if review && !matches!(cmd_name.as_str(), "chat" | "exec" | "continue" | "tui" | "review" | "resume") {
        return Err("--review applies only to exec, continue, review, resume, chat, and tui".into());
    }
    if (staged || sandbox_opt.is_some())
        && !matches!(cmd_name.as_str(), "chat" | "exec" | "continue" | "tui" | "review" | "resume")
    {
        return Err("--staged and --sandbox apply only to exec, continue, review, resume, chat, and tui".into());
    }
    if cmd_name == "continue" && agent != "default" {
        return Err("--agent applies only when starting a new session".into());
    }
    if plain && !matches!(cmd_name.as_str(), "chat" | "resume") {
        return Err("--plain applies only to chat and resume".into());
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
    let sandbox = sandbox_opt.unwrap_or_else(|| {
        if staged {
            SandboxMode::Staged
        } else if cmd_name == "review" {
            SandboxMode::ReadOnly
        } else {
            SandboxMode::Direct
        }
    });

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
        "provider" => {
            if global.session.is_some() || global.workspace.is_some() || agent != "default" {
                return Err(
                    "provider manages user-level configuration and does not take session, workspace or agent options"
                        .into(),
                );
            }
            let pos: Vec<&str> = pos_args.iter().map(String::as_str).collect();
            let action = match pos.as_slice() {
                [] | ["list"] => {
                    if has_provider_flags {
                        return Err("provider flags apply only to `provider add NAME`".into());
                    }
                    ProviderAction::List
                }
                ["show", name] if !name.trim().is_empty() => {
                    if has_provider_flags {
                        return Err("provider flags apply only to `provider add NAME`".into());
                    }
                    ProviderAction::Show((*name).to_owned())
                }
                ["remove" | "rm", name] if !name.trim().is_empty() => {
                    if has_provider_flags {
                        return Err("provider flags apply only to `provider add NAME`".into());
                    }
                    ProviderAction::Remove((*name).to_owned())
                }
                ["default"] => {
                    if has_provider_flags {
                        return Err("provider flags apply only to `provider add NAME`".into());
                    }
                    ProviderAction::DefaultGet
                }
                ["default", target] if !target.trim().is_empty() => {
                    if has_provider_flags {
                        return Err("provider flags apply only to `provider add NAME`".into());
                    }
                    ProviderAction::DefaultSet((*target != "auto").then(|| (*target).to_owned()))
                }
                ["add", name] if !name.trim().is_empty() => {
                    let (Some(wire), Some(base_url)) = (wire, base_url) else {
                        return Err("provider add requires --wire and --base-url".into());
                    };
                    ProviderAction::Add {
                        name: (*name).to_owned(),
                        spec: CustomProviderSpec {
                            wire,
                            base_url,
                            api_key_env,
                            default_model: model_opt,
                            context_window,
                            tool_choice,
                            max_tokens,
                            replay_reasoning: replay_reasoning.then_some(true),
                        },
                        set_default: make_default,
                    }
                }
                _ => {
                    return Err(
                        "use teamagents provider [list|show NAME|add NAME --wire ... --base-url ...|remove NAME|default [NAME|auto]]"
                            .into(),
                    )
                }
            };
            Command::Provider(action)
        }
        "exec" => Command::Exec(ExecOptions { agent, review, sandbox, check, detach, timeout, jsonl, args: pos_args }),
        "continue" => {
            if pos_args.len() < 2 || pos_args[0].trim().is_empty() {
                return Err("continue requires TASK_ID and a prompt (or - for stdin)".into());
            }
            let mut iter = pos_args.into_iter();
            let id = iter.next().unwrap();
            let args = iter.collect();
            Command::Continue { id, opt: ExecOptions { agent, review, sandbox, check, detach, timeout, jsonl, args } }
        }
        "review" => {
            let target = if let Some(b) = base_ref {
                ReviewTarget::Base(b)
            } else if let Some(c) = commit_ref {
                ReviewTarget::Commit(c)
            } else {
                ReviewTarget::Uncommitted
            };
            Command::Review {
                target,
                opt: ExecOptions { agent, review, sandbox, check, detach, timeout, jsonl, args: pos_args },
            }
        }
        "resume" => {
            let mut iter = pos_args.into_iter();
            let id = iter.next().filter(|s| !s.trim().is_empty());
            let args: Vec<String> = iter.collect();
            Command::Resume { id, opt: ExecOptions { agent, review, sandbox, check, detach, timeout, jsonl, args } }
        }
        "steer" => {
            if pos_args.len() < 2 || pos_args[0].trim().is_empty() || pos_args[1..].iter().all(|s| s.trim().is_empty())
            {
                return Err("steer requires TASK_ID and nonempty TEXT".into());
            }
            Command::Steer { id: pos_args[0].clone(), text: pos_args[1..].join(" ") }
        }
        "compact" => {
            if pos_args.len() != 1 || pos_args[0].trim().is_empty() {
                return Err("compact requires exactly one TASK_ID".into());
            }
            Command::Compact { id: pos_args.into_iter().next().unwrap() }
        }
        "diff" => {
            if !pos_args.is_empty() {
                return Err("diff takes --base REF or --commit SHA without positional arguments".into());
            }
            Command::Diff { base: base_ref, commit: commit_ref }
        }
        "skills" => {
            if !pos_args.is_empty() {
                return Err("skills does not take positional arguments".into());
            }
            Command::Skills
        }
        "mcp" => {
            let pos: Vec<&str> = pos_args.iter().map(String::as_str).collect();
            let action = match pos.as_slice() {
                [] | ["list"] => {
                    if has_mcp_flags {
                        return Err("MCP server flags apply only to `mcp add NAME`".into());
                    }
                    McpAction::List
                }
                ["remove" | "rm", name] if !name.trim().is_empty() => {
                    if has_mcp_flags {
                        return Err("MCP server flags apply only to `mcp add NAME`".into());
                    }
                    McpAction::Remove((*name).to_owned())
                }
                ["tools"] => {
                    if has_mcp_flags {
                        return Err("MCP server flags apply only to `mcp add NAME`".into());
                    }
                    McpAction::Tools(None)
                }
                ["tools", name] if !name.trim().is_empty() => {
                    if has_mcp_flags {
                        return Err("MCP server flags apply only to `mcp add NAME`".into());
                    }
                    McpAction::Tools(Some((*name).to_owned()))
                }
                ["add", name] if !name.trim().is_empty() => {
                    let spec = match (mcp_command, mcp_url) {
                        (Some(_), Some(_)) => return Err("mcp add accepts either --command or --url, not both".into()),
                        (Some(command), None) => {
                            if mcp_bearer_env.is_some() {
                                return Err("--bearer-token-env applies only with --url".into());
                            }
                            McpServerSpec::Stdio { command, args: mcp_args, env_vars: mcp_env }
                        }
                        (None, Some(url)) => {
                            if !mcp_args.is_empty() || !mcp_env.is_empty() {
                                return Err("--arg and --env apply only with --command".into());
                            }
                            McpServerSpec::Http { url, bearer_token_env: mcp_bearer_env }
                        }
                        (None, None) => return Err("mcp add requires either --command CMD or --url URL".into()),
                    };
                    McpAction::Add { name: (*name).to_owned(), spec }
                }
                _ => {
                    return Err(
                        "use teamagents mcp [list|add NAME --command CMD|--url URL|remove NAME|tools [NAME]]".into()
                    )
                }
            };
            Command::Mcp(action)
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
                "chat" => Command::Chat { agent, review, sandbox, plain },
                "tui" => Command::Tui { agent, review, sandbox },
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
    for key in ["policy", "sandbox", "verification", "check", "tool_state"] {
        if let Some(value) = task[key].as_str() {
            out.push_str(&format!("\n  {key}: {value}"));
        }
    }
    if let Some(n) = task["pending_steerings"].as_u64().filter(|&n| n > 0) {
        out.push_str(&format!("\n  pending_steerings: {n}"));
    }
    if let Some(plan) = task["plan"].as_array().filter(|rows| !rows.is_empty()) {
        let done = plan.iter().filter(|s| s["status"] == "completed").count();
        out.push_str(&format!("\n  Plan ({done}/{} completed):", plan.len()));
        for step in plan {
            let mark = match step["status"].as_str() {
                Some("completed") => "x",
                Some("in_progress") => ">",
                _ => " ",
            };
            out.push_str(&format!("\n    [{mark}] {}", step["step"].as_str().unwrap_or("")));
        }
    }
    if let Some(live) = task["live"].as_object() {
        let phase = live.get("phase").and_then(Value::as_str).unwrap_or("active");
        out.push_str(&format!("\n  Live ({phase}):"));
        if let Some(tools) = live.get("active_tools").and_then(Value::as_array).filter(|t| !t.is_empty()) {
            let names: Vec<_> = tools.iter().filter_map(Value::as_str).collect();
            if !names.is_empty() {
                out.push_str(&format!(" tools=[{}]", names.join(", ")));
            }
        }
        if let Some(c) = live.get("commentary").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            out.push_str(&format!("\n    {}", preview(c, 160)));
        }
        if let Some(tail) = live.get("shell_tail").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            out.push_str(&format!("\n    $ {}", preview(tail, 160)));
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
    if let Some(usage) = task.get("usage").filter(|u| u["input_tokens"].as_u64().unwrap_or(0) > 0) {
        let input = usage["input_tokens"].as_u64().unwrap_or(0);
        let cached = usage["cached_tokens"].as_u64().unwrap_or(0);
        let output = usage["output_tokens"].as_u64().unwrap_or(0);
        let pct = usage["cache_hit_rate"].as_f64().unwrap_or(0.0) * 100.0;
        out.push_str(&format!("\n  tokens: {input} in ({cached} cached, {pct:.1}% hit), {output} out"));
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
        "steer" => format!("Queued steering note for {}", display_value(&value["root"])),
        "compact" => format!(
            "Compacted {} middle messages on node {}",
            value["messages"].as_u64().unwrap_or(0),
            display_value(&value["task"])
        ),
        "diff" => display_value(&value["diff"]),
        "skills" => {
            let Some(rows) = value["skills"].as_array() else {
                return value.to_string();
            };
            if rows.is_empty() {
                return "No local SKILL.md skills discovered.".into();
            }
            rows.iter()
                .map(|s| {
                    format!(
                        "{} ({}) — {} [{}]",
                        s["name"].as_str().unwrap_or("?"),
                        s["scope"].as_str().unwrap_or("?"),
                        s["description"].as_str().unwrap_or(""),
                        s["path"].as_str().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        "mcp" => {
            let mut lines = Vec::new();
            if let Some(path) = value["config_path"].as_str() {
                lines.push(format!("MCP config: {path}"));
            }
            match value["servers"].as_array() {
                Some(rows) if !rows.is_empty() => {
                    for s in rows {
                        lines.push(format!(
                            "  {} ({}) -> {} [{}]",
                            s["name"].as_str().unwrap_or("?"),
                            s["transport"].as_str().unwrap_or("?"),
                            s["target"].as_str().unwrap_or("?"),
                            if s["ready"] == true { "ready" } else { s["status"].as_str().unwrap_or("not ready") }
                        ));
                    }
                }
                _ => lines.push("  No MCP servers configured.".into()),
            }
            lines.join("\n")
        }
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
    let receipt = client.submit_full(&opt.agent, &prompt(&opt.args)?, opt.review, opt.check.as_deref(), opt.sandbox)?;
    wait_for_root(client, receipt, "submit", &opt, machine)
}

fn continue_cmd(client: Client, id: &str, opt: ExecOptions, machine: bool) -> Result<()> {
    let receipt = client.continue_full(id, &prompt(&opt.args)?, opt.review, opt.check.as_deref(), opt.sandbox)?;
    wait_for_root(client, receipt, "continue", &opt, machine)
}

pub(crate) fn build_review_prompt(target_desc: &str, custom: Option<&str>, diff: &str) -> String {
    let mut prompt = format!(
        "Perform a thorough, read-only code review of {target_desc}.\nFocus on correctness bugs, security/safety risks, missing edge cases, concurrency/durability issues, and regressions. Cite exact file paths, line numbers, severity (P0-P3), and concrete fixes."
    );
    if let Some(extra) = custom {
        if !extra.trim().is_empty() {
            prompt.push_str(&format!("\n\nAdditional reviewer focus:\n{extra}"));
        }
    }
    prompt.push_str(&format!("\n\nObserved git diff/status snapshot:\n```diff\n{diff}\n```"));
    prompt
}

fn review_cmd(client: Client, target: ReviewTarget, opt: ExecOptions, machine: bool) -> Result<()> {
    let (base, commit, target_desc) = match &target {
        ReviewTarget::Uncommitted => (None, None, "uncommitted workspace changes".to_owned()),
        ReviewTarget::Base(r) => (Some(r.as_str()), None, format!("changes against base {r}")),
        ReviewTarget::Commit(c) => (None, Some(c.as_str()), format!("commit {c}")),
    };
    let diff = client.diff(base, commit)?;
    let custom = if opt.args.is_empty() { None } else { Some(opt.args.join(" ")) };
    let prompt = build_review_prompt(&target_desc, custom.as_deref(), &diff);
    let receipt = client.submit_full(&opt.agent, &prompt, opt.review, opt.check.as_deref(), opt.sandbox)?;
    wait_for_root(client, receipt, "submit", &opt, machine)
}

fn emit_jsonl_event(kind: &str, payload: Value) -> Result<()> {
    let mut obj = match payload {
        Value::Object(m) => m,
        other => {
            let mut m = serde_json::Map::new();
            m.insert("data".into(), other);
            m
        }
    };
    obj.insert("type".into(), json!(kind));
    output(&Value::Object(obj).to_string())
}

fn wait_for_root(client: Client, receipt: Value, op: &str, opt: &ExecOptions, machine: bool) -> Result<()> {
    if opt.detach {
        return output(&render(op, &receipt, machine));
    }
    let id = field(&receipt, "id")?.to_owned();
    if opt.jsonl {
        emit_jsonl_event("submitted", receipt.clone())?;
    } else if !machine {
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
    let mut last_live = Value::Null;
    let mut last_plan = Value::Null;
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
        let status = field(&task, "status")?.to_owned();
        if opt.jsonl {
            if !task["live"].is_null() && task["live"] != last_live {
                last_live = task["live"].clone();
                emit_jsonl_event("live", json!({"id": id, "live": last_live}))?;
            }
            if !task["plan"].is_null() && task["plan"] != last_plan {
                last_plan = task["plan"].clone();
                emit_jsonl_event("plan", json!({"id": id, "plan": last_plan}))?;
            }
        }
        if status == "input" {
            if opt.jsonl {
                emit_jsonl_event("input", task)?;
            } else {
                output(&render("status", &task, machine))?;
            }
            return Err(format!("input required; resume with teamagents answer {id} TEXT"));
        }
        if is_terminal_str(&status) {
            if opt.jsonl {
                emit_jsonl_event("completed", task)?;
            } else {
                output(&render("status", &task, machine))?;
            }
            return if status == "done" { Ok(()) } else { Err(format!("task {id} {status}")) };
        }
        if status == "approval" && last != task {
            if opt.jsonl {
                emit_jsonl_event(
                    "approval",
                    json!({"id": id, "approval_id": task["approval_id"], "action": task["action"]}),
                )?;
            } else if !machine {
                eprintln!("{}", sanitize_terminal(&task_text(&task)));
            }
        }
        last = task;
    }
    output(&render(if last.get("status").is_some() { "status" } else { "submit" }, &last, machine))?;
    Err(format!("timed out waiting for {id}; reconnect with status {id}"))
}

fn env_var_status(name: &str) -> &'static str {
    if env::var_os(name).filter(|v| !v.is_empty()).is_some() {
        "set"
    } else {
        "not set in current shell"
    }
}

pub(crate) fn format_provider_list_lines(
    active: &str,
    overridden: bool,
    config_path: Option<&str>,
    providers: &[ProviderSummary],
) -> String {
    let mut lines = Vec::new();
    let mut header = format!("Active provider: {active}");
    if overridden {
        header.push_str(" (TEAMAGENTS_PROVIDER env override)");
    }
    lines.push(header);
    if let Some(path) = config_path {
        lines.push(format!("Config file: {path}"));
    }
    for p in providers {
        let marker = if p.name == active { "*" } else { " " };
        let origin = if p.builtin { "built-in" } else { "custom" };
        let readiness = if p.ready { "ready".to_owned() } else { format!("not ready: {}", p.status) };
        let mut line = format!("{marker} {} ({}, {origin}) [{readiness}]", p.name, p.wire);
        if let Some(m) = p.default_model.as_deref() {
            line.push_str(&format!(" model={m}"));
        }
        if let Some(env_name) = p.api_key_env.as_deref() {
            line.push_str(&format!(" key_env={env_name}"));
        }
        if let Some(url) = p.base_url.as_deref() {
            line.push_str(&format!(" url={url}"));
        }
        lines.push(line);
    }
    lines.push("Switch live with `/model provider NAME [MODEL [EFFORT]]`.".into());
    lines.join("\n")
}

fn provider_summary_from_info(p: &ProviderInfo) -> ProviderSummary {
    ProviderSummary {
        name: p.name.clone(),
        wire: p.wire.clone(),
        builtin: p.builtin,
        base_url: p.base_url.clone(),
        api_key_env: p.api_key_env.clone(),
        default_model: p.default_model.clone(),
        context_window: p.context_window,
        tool_choice: p.tool_choice.clone(),
        ready: p.ready,
        status: p.status.clone(),
    }
}

fn format_provider_detail(p: &ProviderSummary) -> String {
    let mut lines = vec![
        format!("Provider: {} ({}, {})", p.name, p.wire, if p.builtin { "built-in" } else { "custom" }),
        format!("Status: {}", p.status),
        format!("Tool choice: {}", p.tool_choice),
    ];
    if let Some(url) = p.base_url.as_deref() {
        lines.push(format!("Base URL: {url}"));
    }
    if let Some(env_name) = p.api_key_env.as_deref() {
        lines.push(format!("API key env var: {env_name} ({})", env_var_status(env_name)));
    }
    if let Some(model) = p.default_model.as_deref() {
        lines.push(format!("Default model: {model}"));
    }
    if let Some(ctx) = p.context_window {
        lines.push(format!("Context window: {ctx}"));
    }
    lines.join("\n")
}

/// Parse `/provider add NAME ...` from either `--flag` options or positional shorthand.
pub(crate) fn parse_provider_add_tokens(tokens: &[&str]) -> Result<(String, CustomProviderSpec, bool)> {
    let usage = "use /provider add NAME --wire WIRE --base-url URL [--api-key-env VAR] [--model ID] [--context-window N] [--tool-choice required|auto] [--max-tokens N] [--replay-reasoning] [--default] OR /provider add NAME WIRE BASE_URL [API_KEY_ENV [MODEL]]";
    let Some((&name, rest)) = tokens.split_first() else {
        return Err(usage.into());
    };
    if name.trim().is_empty() || name.starts_with('-') {
        return Err(usage.into());
    }
    if rest.is_empty() {
        return Err(usage.into());
    }
    if !rest[0].starts_with('-') {
        if rest.len() < 2 || rest.len() > 4 {
            return Err(usage.into());
        }
        let wire = WireProtocol::parse_custom(rest[0])?.as_str().to_owned();
        let base_url = rest[1].to_owned();
        let api_key_env = rest.get(2).filter(|s| **s != "-").map(|s| (*s).to_owned());
        let default_model = rest.get(3).map(|s| (*s).to_owned());
        return Ok((
            name.to_owned(),
            CustomProviderSpec {
                wire,
                base_url,
                api_key_env,
                default_model,
                context_window: None,
                tool_choice: None,
                max_tokens: None,
                replay_reasoning: None,
            },
            false,
        ));
    }
    let mut wire = None;
    let mut base_url = None;
    let mut api_key_env = None;
    let mut default_model = None;
    let mut context_window = None;
    let mut tool_choice = None;
    let mut max_tokens = None;
    let mut replay_reasoning = false;
    let mut set_default = false;
    let mut i = 0;
    while i < rest.len() {
        let flag = rest[i];
        let next = |idx: usize| rest.get(idx + 1).copied().ok_or_else(|| format!("{flag} requires a value"));
        match flag {
            "--wire" => {
                wire = Some(WireProtocol::parse_custom(next(i)?)?.as_str().to_owned());
                i += 2;
            }
            "--base-url" => {
                base_url = Some(next(i)?.to_owned());
                i += 2;
            }
            "--api-key-env" => {
                api_key_env = Some(next(i)?.to_owned());
                i += 2;
            }
            "--model" => {
                default_model = Some(next(i)?.to_owned());
                i += 2;
            }
            "--context-window" => {
                let n: u64 = next(i)?.parse().map_err(|_| "--context-window requires a positive integer")?;
                if n == 0 {
                    return Err("--context-window requires a positive integer".into());
                }
                context_window = Some(n);
                i += 2;
            }
            "--tool-choice" => {
                tool_choice = Some(ToolChoiceMode::parse(next(i)?)?);
                i += 2;
            }
            "--max-tokens" => {
                let n: u32 = next(i)?.parse().map_err(|_| "--max-tokens requires a positive integer")?;
                if n == 0 {
                    return Err("--max-tokens requires a positive integer".into());
                }
                max_tokens = Some(n);
                i += 2;
            }
            "--replay-reasoning" => {
                replay_reasoning = true;
                i += 1;
            }
            "--default" => {
                set_default = true;
                i += 1;
            }
            _ => return Err(format!("unknown option for /provider add: {flag}")),
        }
    }
    let (Some(wire), Some(base_url)) = (wire, base_url) else {
        return Err("provider add requires --wire and --base-url".into());
    };
    Ok((
        name.to_owned(),
        CustomProviderSpec {
            wire,
            base_url,
            api_key_env,
            default_model,
            context_window,
            tool_choice,
            max_tokens,
            replay_reasoning: replay_reasoning.then_some(true),
        },
        set_default,
    ))
}

fn provider_cli_command(action: ProviderAction, machine: bool) -> Result<()> {
    match action {
        ProviderAction::List => {
            let list = model::list_providers()?;
            let default_name = model::default_provider_name()?;
            let env_override = model::explicit_env_provider()?;
            let active = env_override.clone().unwrap_or_else(|| default_name.clone());
            let config_path = model::providers_config_path_display();
            if machine {
                return output(
                    &json!({
                        "active": active,
                        "default": default_name,
                        "overridden": env_override.is_some(),
                        "config_path": config_path,
                        "providers": list,
                    })
                    .to_string(),
                );
            }
            let summaries: Vec<_> = list.iter().map(provider_summary_from_info).collect();
            output(&format_provider_list_lines(&active, env_override.is_some(), config_path.as_deref(), &summaries))
        }
        ProviderAction::Show(name) => {
            let list = model::list_providers()?;
            let Some(info) = list.iter().find(|p| p.name == name) else {
                return Err(format!("unknown provider {name:?}"));
            };
            if machine {
                return output(&serde_json::to_string(info).map_err(|e| e.to_string())?);
            }
            output(&format_provider_detail(&provider_summary_from_info(info)))
        }
        ProviderAction::Add { name, spec, set_default } => {
            let api_key_env = spec.api_key_env.clone();
            let wire = spec.wire.clone();
            let base_url = spec.base_url.clone();
            let path = model::upsert_provider(&name, spec, set_default)?;
            if machine {
                return output(
                    &json!({
                        "saved": name,
                        "wire": wire,
                        "base_url": base_url,
                        "api_key_env": api_key_env,
                        "default": set_default,
                        "path": path.display().to_string(),
                    })
                    .to_string(),
                );
            }
            let mut msg = format!("Saved provider '{name}' ({wire} @ {base_url}) to {}.", path.display());
            if let Some(env_name) = api_key_env.as_deref() {
                msg.push_str(&format!("\nAPI key env {env_name}: {}.", env_var_status(env_name)));
            }
            if set_default {
                msg.push_str("\nSet as default provider.");
            }
            output(&msg)
        }
        ProviderAction::Remove(name) => {
            let path = model::remove_provider(&name)?;
            if machine {
                return output(&json!({"removed": name, "path": path.display().to_string()}).to_string());
            }
            output(&format!("Removed provider '{name}' from {}.", path.display()))
        }
        ProviderAction::DefaultGet => {
            let default_name = model::default_provider_name()?;
            let env_override = model::explicit_env_provider()?;
            let config_path = model::providers_config_path_display();
            if machine {
                return output(
                    &json!({
                        "default": default_name,
                        "overridden": env_override.is_some(),
                        "env_provider": env_override,
                        "config_path": config_path,
                    })
                    .to_string(),
                );
            }
            if let Some(env_name) = env_override {
                output(&format!(
                    "Configured default provider: {default_name} (current shell overridden by TEAMAGENTS_PROVIDER={env_name})"
                ))
            } else {
                output(&format!("Default provider: {default_name}"))
            }
        }
        ProviderAction::DefaultSet(target) => {
            let path = model::set_default_provider(target.as_deref())?;
            let resolved = model::default_provider_name()?;
            if machine {
                return output(
                    &json!({
                        "configured_default": target,
                        "effective_default": resolved,
                        "path": path.display().to_string(),
                    })
                    .to_string(),
                );
            }
            match target {
                Some(name) => output(&format!("Default provider set to '{name}' in {}.", path.display())),
                None => output(&format!(
                    "Cleared configured default provider in {} (effective auto default: {resolved}).",
                    path.display()
                )),
            }
        }
    }
}

fn provider_chat_command(client: &Client, args: &str, machine: bool) -> Result<()> {
    let words: Vec<_> = args.split_whitespace().collect();
    match words.as_slice() {
        [] | ["list"] => {
            let (active, overridden, config_path, providers) = client.providers()?;
            if machine {
                return output(
                    &json!({
                        "active": active,
                        "overridden": overridden,
                        "config_path": config_path,
                        "providers": providers,
                    })
                    .to_string(),
                );
            }
            output(&format_provider_list_lines(&active, overridden, config_path.as_deref(), &providers))
        }
        ["show", name] => {
            let (_, _, _, providers) = client.providers()?;
            let Some(found) = providers.iter().find(|p| p.name == *name) else {
                return Err(format!("unknown provider {name:?}"));
            };
            if machine {
                return output(&serde_json::to_string(found).map_err(|e| e.to_string())?);
            }
            output(&format_provider_detail(found))
        }
        ["use", rest @ ..] if !rest.is_empty() => {
            let joined = format!("provider {}", rest.join(" "));
            model_command(client, &joined, machine)
        }
        ["default"] => provider_cli_command(ProviderAction::DefaultGet, machine),
        ["default", target] => {
            provider_cli_command(ProviderAction::DefaultSet((*target != "auto").then(|| (*target).to_owned())), machine)
        }
        ["remove" | "rm", name] => provider_cli_command(ProviderAction::Remove((*name).to_owned()), machine),
        ["add", rest @ ..] => {
            let (name, spec, set_default) = parse_provider_add_tokens(rest)?;
            let switch_hint = name.clone();
            provider_cli_command(ProviderAction::Add { name, spec, set_default }, machine)?;
            if !machine {
                output(&format!("Use `/model provider {switch_hint}` to switch to it now."))?;
            }
            Ok(())
        }
        _ => Err("use /provider [list|show NAME|use NAME|add NAME ...|remove NAME|default [NAME|auto]]".into()),
    }
}

fn model_command(client: &Client, args: &str, machine: bool) -> Result<()> {
    let words: Vec<_> = args.split_whitespace().collect();
    let usage = "use /model, /model list, /model providers, /model provider NAME [MODEL [EFFORT]], /model MODEL [EFFORT], /model effort EFFORT or /model default";
    if words == ["providers"] {
        let (active, overridden, config_path, providers) = client.providers()?;
        if machine {
            return output(
                &json!({
                    "active": active,
                    "overridden": overridden,
                    "config_path": config_path,
                    "providers": providers,
                })
                .to_string(),
            );
        }
        return output(&format_provider_list_lines(&active, overridden, config_path.as_deref(), &providers));
    }
    if words == ["list"] {
        let cfg = client.model_get()?;
        let models = client.model_list(None)?;
        if !machine {
            return if models.is_empty() {
                output(&format!("No visible models for provider {}.", cfg.provider))
            } else {
                output(&models.join("\n"))
            };
        }
        return output(
            &json!({
                "provider": cfg.provider,
                "model": cfg.model,
                "effort": cfg.effort,
                "models": models,
            })
            .to_string(),
        );
    }
    let cfg = match words.as_slice() {
        [] => client.model_get()?,
        ["default"] => client.model_set(None, Some(None), Some(None))?,
        ["effort", effort] => {
            let e = (*effort != "default").then_some(*effort);
            client.model_set(None, None, Some(e))?
        }
        ["provider", target] => {
            let p = (*target != "default").then_some(*target);
            client.model_set(Some(p), None, None)?
        }
        ["provider", target, name] => {
            let p = (*target != "default").then_some(*target);
            let m = (*name != "default").then_some(*name);
            client.model_set(Some(p), Some(m), Some(None))?
        }
        ["provider", target, name, effort] => {
            let p = (*target != "default").then_some(*target);
            let m = (*name != "default").then_some(*name);
            let e = (*effort != "default").then_some(*effort);
            client.model_set(Some(p), Some(m), Some(e))?
        }
        ["provider" | "providers" | "effort" | "list" | "default", ..] => return Err(usage.into()),
        args => {
            let args = args.strip_prefix(&["set"]).unwrap_or(args);
            let (name, effort) = match args {
                [name] => (*name, Some(None)),
                [name, effort] => (*name, Some((*effort != "default").then_some(*effort))),
                _ => return Err(usage.into()),
            };
            let m = (name != "default").then_some(name);
            client.model_set(None, Some(m), effort)?
        }
    };
    if machine {
        return output(
            &json!({
                "provider": cfg.provider,
                "model": cfg.model,
                "effort": cfg.effort,
            })
            .to_string(),
        );
    }
    let fallback = if cfg.provider == "chatgpt" { "automatic (first visible plan model)" } else { "unset" };
    let model = cfg.model.as_deref().unwrap_or(fallback);
    let effort = cfg.effort.as_deref().unwrap_or("provider default");
    let mut text = format!("New tasks use {} | model: {model} | reasoning effort: {effort}", cfg.provider);
    if words.is_empty() {
        text.push_str("\n/model MODEL [EFFORT]   /model effort EFFORT   /model default");
        text.push_str("\n/model providers   /model provider NAME [MODEL [EFFORT]]");
        text.push_str(&format!("\nEffort: default, {} (support varies by model).", daemon::EFFORTS.join(", ")));
        text.push_str("\nUse /model list to query available models for the active provider.");
    }
    output(&text)
}

fn mcp_cli_command(action: McpAction, machine: bool) -> Result<()> {
    match action {
        McpAction::List => {
            let config_path = mcp::mcp_config_path_display();
            let servers = mcp::list_servers()?;
            let val = json!({"config_path": config_path, "servers": servers});
            output(&render("mcp", &val, machine))
        }
        McpAction::Add { name, spec } => {
            let path = mcp::upsert_server(&name, spec)?;
            if machine {
                output(&json!({"saved": name, "path": path.display().to_string()}).to_string())
            } else {
                output(&format!("Saved MCP server '{name}' to {}.", path.display()))
            }
        }
        McpAction::Remove(name) => {
            let path = mcp::remove_server(&name)?;
            if machine {
                output(&json!({"removed": name, "path": path.display().to_string()}).to_string())
            } else {
                output(&format!("Removed MCP server '{name}' from {}.", path.display()))
            }
        }
        McpAction::Tools(server) => {
            let tools = mcp::list_tools(server.as_deref())?;
            if machine {
                return output(&json!({"server": server, "tools": tools}).to_string());
            }
            output(&mcp::list_tools_text(server.as_deref())?)
        }
    }
}

fn chat_plain(
    client: &Client,
    initial_agent: &str,
    review: bool,
    initial_sandbox: SandboxMode,
    machine: bool,
) -> Result<()> {
    let mut agent = initial_agent.to_owned();
    let mut sandbox = initial_sandbox;
    let mut active_root: Option<String> = None;
    if !machine && io::stdin().is_terminal() {
        let mode = if review { "individual action review" } else { "unattended" };
        output(&format!(
            "New session with {agent} ({mode}, {}). Enter a task, or /help for commands.",
            sandbox.as_str()
        ))?;
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
    let submit = |agent: &str, text: &str, sandbox: SandboxMode| -> Result<String> {
        let receipt = client.submit_full(agent, text, review, None, sandbox)?;
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
                    ("provider", _, _) => provider_chat_command(client, args, machine),
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
                    ("sandbox", None, None) => output(&if machine {
                        json!({"sandbox": sandbox.as_str()}).to_string()
                    } else {
                        format!("Sandbox mode: {}.", sandbox.as_str())
                    }),
                    ("sandbox", Some(mode_str), None) => {
                        sandbox = SandboxMode::parse(mode_str)?;
                        output(&if machine {
                            json!({"sandbox": sandbox.as_str()}).to_string()
                        } else {
                            format!("Sandbox mode set to {}.", sandbox.as_str())
                        })
                    }
                    ("staged", None, None) => {
                        sandbox = if sandbox.is_staged() { SandboxMode::Direct } else { SandboxMode::Staged };
                        output(&if machine {
                            json!({"staged": sandbox.is_staged(), "sandbox": sandbox.as_str()}).to_string()
                        } else {
                            format!("Sandbox mode: {}.", sandbox.as_str())
                        })
                    }
                    ("staged", Some(mode @ ("on" | "off")), None) => {
                        sandbox = if mode == "on" { SandboxMode::Staged } else { SandboxMode::Direct };
                        output(&if machine {
                            json!({"staged": sandbox.is_staged(), "sandbox": sandbox.as_str()}).to_string()
                        } else {
                            format!("Sandbox mode set to {}.", sandbox.as_str())
                        })
                    }
                    ("plan", Some(_), _) => {
                        let plan_prompt = format!(
                            "Create a concise, verified implementation plan (read-only analysis; do not modify files). Use update_plan to publish concrete steps.\n\nRequest:\n{args}"
                        );
                        match submit(&agent, &plan_prompt, SandboxMode::ReadOnly) {
                            Ok(id) => {
                                active_root = Some(id);
                                Ok(())
                            }
                            Err(e) => Err(e),
                        }
                    }
                    ("review", _, _) => {
                        let mut rparts = args.split_whitespace();
                        let base_ref = match (rparts.next(), rparts.next()) {
                            (Some("code"), b) => b,
                            (Some(b), _) => Some(b),
                            (None, _) => None,
                        };
                        let diff = client.diff(base_ref, None)?;
                        let prompt = format!(
                            "Perform a read-only code review focusing on bugs, security risks, missing checks, and regressions.\n\n```diff\n{diff}\n```"
                        );
                        match submit(&agent, &prompt, SandboxMode::ReadOnly) {
                            Ok(id) => {
                                active_root = Some(id);
                                Ok(())
                            }
                            Err(e) => Err(e),
                        }
                    }
                    ("steer", Some(_), _) => {
                        let (target_id, steer_text) = if let Some(root_id) = active_root.as_deref() {
                            (root_id.to_owned(), args.to_owned())
                        } else if let Some((id_part, rest)) = args.split_once(char::is_whitespace) {
                            (id_part.to_owned(), rest.trim().to_owned())
                        } else {
                            return Err("use /steer TEXT (after starting a session) or /steer TASK_ID TEXT".into());
                        };
                        let val = client.steer(&target_id, &steer_text)?;
                        output(&render("steer", &val, machine))
                    }
                    ("compact", maybe_id, None) => {
                        let target_id = maybe_id
                            .map(str::to_owned)
                            .or_else(|| active_root.clone())
                            .ok_or("start or select a session first, or pass /compact TASK_ID")?;
                        let val = client.compact(&target_id)?;
                        output(&render("compact", &val, machine))
                    }
                    ("diff", base_ref, None) => {
                        let text = client.diff(base_ref, None)?;
                        let val = json!({"diff": text});
                        output(&render("diff", &val, machine))
                    }
                    ("skills", None, None) => {
                        let skills = client.skills()?;
                        output(&render("skills", &json!({"skills": skills}), machine))
                    }
                    ("mcp", None, None) => {
                        let (config_path, servers) = client.mcp_status()?;
                        output(&render("mcp", &json!({"config_path": config_path, "servers": servers}), machine))
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
                    ("new", Some(_), _) => match submit(&agent, args, sandbox) {
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
                            match client.continue_full(root_id, line, review, None, sandbox) {
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
                            match submit(&agent, line, sandbox) {
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
        Command::Provider(action) => return provider_cli_command(action, global.json),
        Command::Mcp(action) => return mcp_cli_command(action, global.json),
        _ => {}
    }

    let client = Client::new(global.session.as_deref(), global.workspace.as_deref())?;
    let interactive_tty = io::stdin().is_terminal() && io::stdout().is_terminal();
    match cmd {
        Command::Daemon => daemon::serve(&client.session, &client.workspace),
        Command::Tui { agent, review, sandbox } => {
            if !interactive_tty {
                return Err(
                    "tui requires an interactive terminal; use `teamagents chat --plain` or `teamagents exec`".into()
                );
            }
            crate::tui::run(client, agent, review, sandbox, None)
        }
        Command::Chat { agent, review, sandbox, plain } => {
            if !global.json && !plain && interactive_tty {
                crate::tui::run(client, agent, review, sandbox, None)
            } else {
                chat_plain(&client, &agent, review, sandbox, global.json)
            }
        }
        Command::Exec(opt) => exec(client, opt, global.json),
        Command::Continue { id, opt } => continue_cmd(client, &id, opt, global.json),
        Command::Review { target, opt } => review_cmd(client, target, opt, global.json),
        Command::Resume { id, opt } => {
            let target_id = if let Some(explicit) = id {
                explicit
            } else {
                let tasks = client.tasks()?;
                tasks
                    .iter()
                    .rev()
                    .find(|t| t.is_root() && t.continuation_node.is_some())
                    .or_else(|| tasks.iter().rev().find(|t| t.is_root()))
                    .map(|t| t.id.clone())
                    .ok_or("no existing sessions in this workspace to resume")?
            };
            if !opt.args.is_empty() {
                continue_cmd(client, &target_id, opt, global.json)
            } else if !global.json && interactive_tty {
                crate::tui::run(client, opt.agent, opt.review, opt.sandbox, Some(target_id))
            } else {
                command(&client, "status", Some(&target_id), global.json)
            }
        }
        Command::Steer { id, text } => {
            let value = client.steer(&id, &text)?;
            output(&render("steer", &value, global.json))
        }
        Command::Compact { id } => {
            let value = client.compact(&id)?;
            output(&render("compact", &value, global.json))
        }
        Command::Diff { base, commit } => {
            let text = client.diff(base.as_deref(), commit.as_deref())?;
            output(&render("diff", &json!({"diff": text}), global.json))
        }
        Command::Skills => {
            let skills = client.skills()?;
            output(&render("skills", &json!({"skills": skills}), global.json))
        }
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
        Command::Help | Command::Version | Command::Auth(_) | Command::Provider(_) | Command::Mcp(_) => {
            unreachable!()
        }
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
            Command::Tui { ref agent, review: true, sandbox: SandboxMode::Staged } if agent == "builder"
        ));
        assert!(matches!(
            options(&["exec", "--staged", "--jsonl", "run"]).unwrap().command,
            Command::Exec(ref opt) if opt.sandbox == SandboxMode::Staged && opt.jsonl
        ));
        assert!(matches!(
            options(&["exec", "--sandbox", "read-only", "inspect"]).unwrap().command,
            Command::Exec(ref opt) if opt.sandbox == SandboxMode::ReadOnly
        ));
        assert!(matches!(
            options(&["review", "--base", "main", "check security"]).unwrap().command,
            Command::Review { target: ReviewTarget::Base(ref b), ref opt }
                if b == "main" && opt.sandbox == SandboxMode::ReadOnly && opt.args == vec!["check security"]
        ));
        assert!(matches!(
            options(&["resume", "r1", "next step"]).unwrap().command,
            Command::Resume { id: Some(ref i), ref opt } if i == "r1" && opt.args == vec!["next step"]
        ));
        assert!(matches!(
            options(&["steer", "r1", "focus", "on", "tests"]).unwrap().command,
            Command::Steer { ref id, ref text } if id == "r1" && text == "focus on tests"
        ));
        assert!(matches!(
            options(&["compact", "r1"]).unwrap().command,
            Command::Compact { ref id } if id == "r1"
        ));
        assert!(matches!(
            options(&["diff", "--commit", "HEAD"]).unwrap().command,
            Command::Diff { base: None, commit: Some(ref c) } if c == "HEAD"
        ));
        assert!(matches!(options(&["skills"]).unwrap().command, Command::Skills));
        assert!(matches!(
            options(&["mcp", "add", "srv", "--url", "https://mcp.example.com", "--bearer-token-env", "MCP_TOKEN"]).unwrap().command,
            Command::Mcp(McpAction::Add { ref name, spec: McpServerSpec::Http { ref url, bearer_token_env: Some(ref env) } })
                if name == "srv" && url == "https://mcp.example.com" && env == "MCP_TOKEN"
        ));
        assert!(options(&["exec", "--staged", "--sandbox", "read-only", "bad"]).is_err());
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
            &["tasks", "--wire", "responses"],
            &["provider", "list", "--wire", "responses"],
            &["provider", "--session", "s", "list"],
            &["provider", "add", "myprov"],
            &["provider", "add", "myprov", "--wire", "chatgpt", "--base-url", "https://x.test"],
            &[
                "provider",
                "add",
                "myprov",
                "--wire",
                "responses",
                "--base-url",
                "https://x.test",
                "--context-window",
                "0",
            ],
        ] {
            assert!(options(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn provider_cli_and_chat_add_parsers_validate_flags_and_shorthand() {
        assert!(matches!(options(&["provider"]).unwrap().command, Command::Provider(ProviderAction::List)));
        assert!(matches!(
            options(&["provider", "--json", "show", "deepseek"]).unwrap(),
            Parsed { global: GlobalOptions { json: true, .. }, command: Command::Provider(ProviderAction::Show(ref n)) }
                if n == "deepseek"
        ));
        assert!(matches!(
            options(&[
                "provider",
                "add",
                "byteseek",
                "--wire",
                "responses",
                "--base-url",
                "https://api.byteseek.test/v1/responses",
                "--api-key-env",
                "BYTESEEK_KEY",
                "--model",
                "bs-coder",
                "--context-window",
                "200000",
                "--tool-choice",
                "auto",
                "--max-tokens",
                "8192",
                "--replay-reasoning",
                "--default",
            ])
            .unwrap()
            .command,
            Command::Provider(ProviderAction::Add { ref name, ref spec, set_default: true })
                if name == "byteseek"
                    && spec.wire == "responses"
                    && spec.api_key_env.as_deref() == Some("BYTESEEK_KEY")
                    && spec.default_model.as_deref() == Some("bs-coder")
                    && spec.context_window == Some(200_000)
                    && spec.tool_choice == Some(ToolChoiceMode::Auto)
                    && spec.max_tokens == Some(8192)
                    && spec.replay_reasoning == Some(true)
        ));
        assert!(matches!(
            options(&["provider", "default", "auto"]).unwrap().command,
            Command::Provider(ProviderAction::DefaultSet(None))
        ));
        assert!(matches!(
            options(&["provider", "rm", "byteseek"]).unwrap().command,
            Command::Provider(ProviderAction::Remove(ref n)) if n == "byteseek"
        ));

        let (name, spec, set_def) = parse_provider_add_tokens(&[
            "local-vllm",
            "chat_completions",
            "http://127.0.0.1:8000/v1/chat/completions",
            "-",
            "qwen-coder",
        ])
        .unwrap();
        assert_eq!(name, "local-vllm");
        assert_eq!(spec.wire, "chat_completions");
        assert_eq!(spec.api_key_env, None);
        assert_eq!(spec.default_model.as_deref(), Some("qwen-coder"));
        assert!(!set_def);

        let (name2, spec2, set_def2) = parse_provider_add_tokens(&[
            "claude-proxy",
            "--wire",
            "anthropic",
            "--base-url",
            "https://claude.proxy.test",
            "--api-key-env",
            "PROXY_KEY",
            "--default",
        ])
        .unwrap();
        assert_eq!(name2, "claude-proxy");
        assert_eq!(spec2.wire, "anthropic");
        assert!(set_def2);
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
