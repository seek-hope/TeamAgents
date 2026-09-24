//! teamagents: single Rust entry point — CLI, TUI launcher, session daemon and
//! the headless client (`exec`) that shares the daemon with the TUI.

use std::path::{Path, PathBuf};
use teamagents_engine::{cli, tools, VERSION};

const HELP: &str = "TeamAgents：在终端里与 Leader 协作\n\n\
用法：teamagents [--cwd DIR] [--state-root PATH] [--model KEY] [--full-auto]\n\
  teamagents                          连接当前用户 daemon 的 TUI（不存在则先启动 daemon）\n\
  teamagents exec [--json] [--timeout SEC] \"…\"   同一后端的无头输入\n\
  teamagents daemon [--state-root PATH] [--cwd DIR] [--model KEY] [--full-auto]\n\
  teamagents init [--state-root PATH] 创建配置并准备 v2 状态根\n\
  teamagents doctor [--state-root PATH] 检查配置、密钥、v2 状态根与本机条件\n\
  teamagents version | --version      查看版本\n\
  teamagents --help                   查看帮助\n\n\
团队由 Leader 通过 spawn/delegate/send/wait 建立；旧版 TeamSpec/行模式/会话恢复入口已随 v1 后端退役。\n\
首次使用：teamagents init → 设置密钥环境变量 → teamagents doctor → teamagents。";

fn usage() -> ! {
    eprintln!("{HELP}");
    std::process::exit(2);
}

pub struct Args {
    pub cwd: Option<String>,
    pub resume: Option<String>,
    pub full_auto: bool,
    pub team: Option<String>,
    pub plain: bool,
    pub verbose: bool,
    pub command: Option<String>,
    pub positional: Option<String>,
    pub state_root: Option<String>,
    pub model: Option<String>,
    pub timeout: Option<u64>,
    pub checks: Vec<String>,
    pub exec_json: bool,
    pub dry_run: bool,
    pub history_days: Option<u64>,
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
        verbose: false,
        command: None,
        positional: None,
        state_root: None,
        model: None,
        timeout: None,
        checks: Vec::new(),
        exec_json: false,
        dry_run: false,
        history_days: None,
    };
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--cwd" => {
                if args.cwd.is_some() {
                    usage();
                }
                let v = argv.get(i + 1).cloned().filter(|v| !v.starts_with('-')).unwrap_or_else(|| usage());
                args.cwd = Some(v);
                i += 2;
            }
            "--resume" => {
                if args.resume.is_some() {
                    usage();
                }
                let v = argv.get(i + 1).cloned().filter(|v| !v.starts_with('-')).unwrap_or_else(|| usage());
                args.resume = Some(v);
                i += 2;
            }
            "--team" => {
                if args.team.is_some() {
                    usage();
                }
                let v = argv.get(i + 1).cloned().filter(|v| !v.starts_with('-')).unwrap_or_else(|| usage());
                args.team = Some(v);
                i += 2;
            }
            "--full-auto" => {
                if args.full_auto {
                    usage();
                }
                args.full_auto = true;
                i += 1;
            }
            "--plain" => {
                if args.plain {
                    usage();
                }
                args.plain = true;
                i += 1;
            }
            "-v" | "--verbose" => {
                if args.verbose {
                    usage();
                }
                args.verbose = true;
                i += 1;
            }
            // internal: the controlled shell job runner (§6.2), never user-facing
            "jobs-runner" => {
                if args.command.is_some() {
                    usage();
                }
                args.command = Some(argv[i].clone());
                args.positional = argv.get(i + 1).cloned();
                i += 2;
            }
            "--state-root" => {
                if args.state_root.is_some() {
                    usage();
                }
                let v = argv.get(i + 1).cloned().filter(|v| !v.starts_with('-')).unwrap_or_else(|| usage());
                args.state_root = Some(v);
                i += 2;
            }
            "--model" => {
                if args.model.is_some() {
                    usage();
                }
                let v = argv.get(i + 1).cloned().filter(|v| !v.starts_with('-')).unwrap_or_else(|| usage());
                args.model = Some(v);
                i += 2;
            }
            "daemon" => {
                if args.command.is_some() {
                    usage();
                }
                args.command = Some(argv[i].clone());
                i += 1;
            }
            "serve" | "init" | "doctor" | "validate" | "sessions" | "version" | "exec" => {
                if args.command.is_some() {
                    usage();
                }
                args.command = Some(argv[i].clone());
                if argv[i] == "exec" {
                    args.exec_json = false;
                }
                i += 1;
            }
            "--json" if args.command.as_deref() == Some("exec") => {
                if args.exec_json {
                    usage();
                }
                args.exec_json = true;
                i += 1;
            }
            "--timeout" if args.command.as_deref() == Some("exec") => {
                if args.timeout.is_some() {
                    usage();
                }
                let raw = argv.get(i + 1).cloned().filter(|v| !v.starts_with('-')).unwrap_or_else(|| usage());
                let parsed = raw.parse::<u64>().unwrap_or_else(|_| usage());
                if parsed == 0 {
                    usage();
                }
                args.timeout = Some(parsed);
                i += 2;
            }
            "--check" if args.command.as_deref() == Some("exec") => {
                args.checks.push(argv.get(i + 1).cloned().filter(|v| !v.starts_with('-')).unwrap_or_else(|| usage()));
                i += 2;
            }
            "--history-days" if args.command.as_deref() == Some("sessions") => {
                if args.history_days.is_some() {
                    usage();
                }
                let raw = argv.get(i + 1).cloned().filter(|v| !v.starts_with('-')).unwrap_or_else(|| usage());
                args.history_days = Some(raw.parse::<u64>().unwrap_or_else(|_| usage()));
                i += 2;
            }
            "--days" if args.command.as_deref() == Some("sessions") => {
                if args.timeout.is_some() {
                    usage();
                }
                let raw = argv.get(i + 1).cloned().filter(|v| !v.starts_with('-')).unwrap_or_else(|| usage());
                args.timeout = Some(raw.parse::<u64>().unwrap_or_else(|_| usage()));
                i += 2;
            }
            "--dry-run" if args.command.as_deref() == Some("sessions") => {
                if args.dry_run {
                    usage();
                }
                args.dry_run = true;
                i += 1;
            }
            other if !other.starts_with('-') || other == "-" => {
                if args.positional.is_some() {
                    usage();
                }
                args.positional = Some(argv[i].clone());
                i += 1;
            }
            _ => usage(),
        }
    }
    if args.command.as_deref() == Some("exec") && !args.exec_json {
        usage();
    }
    if args.command.as_deref() == Some("init")
        && (args.positional.is_some()
            || args.cwd.is_some()
            || args.resume.is_some()
            || args.team.is_some()
            || args.plain
            || args.full_auto
            || args.verbose)
    {
        usage();
    }
    args
}

fn find_tui_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("TEAMAGENTS_TUI").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let exe = std::env::current_exe().ok();
    if let Some(dir) = exe.as_ref().and_then(|p| p.parent()) {
        let sibling = dir.join("teamagents-tui");
        if sibling.exists() {
            return Some(sibling);
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
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    tools::which("teamagents-tui")
}

fn tui_search_roots(exe: Option<&std::path::Path>) -> Vec<PathBuf> {
    exe.map(|exe| exe.ancestors().map(Path::to_path_buf).collect()).unwrap_or_default()
}

/// R29 default entry: one daemon per user owns the session; the TUI is a thin
/// client of its socket (§9). The daemon is started detached when no socket is
/// live, so quitting the TUI never stops the session.
fn run_tui(args: &Args) -> i32 {
    let Some(binary) = find_tui_binary() else {
        eprintln!("找不到 teamagents-tui。请将发行包中的 teamagents 和 teamagents-tui 安装在同一目录，或用 TEAMAGENTS_TUI 指定路径。\n源码构建：cargo build --manifest-path tui/Cargo.toml；无头方式用 teamagents exec。");
        return 1;
    };
    let state_root = args.state_root.clone().map(PathBuf::from).unwrap_or_else(teamagents_engine::v2_root);
    let socket = match ensure_daemon(&state_root, args.model.clone()) {
        Ok(socket) => socket,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    let mut command = std::process::Command::new(binary);
    command.arg("--daemon").arg(&socket);
    command.arg("--state-root").arg(&state_root);
    if let Some(cwd) = &args.cwd {
        command.args(["--cwd", cwd]);
    }
    let engine = std::env::current_exe().unwrap_or_default();
    command.env("TEAMAGENTS_ENGINE", engine);
    match command.status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(e) => {
            eprintln!("无法启动 TUI: {e}");
            1
        }
    }
}

/// `teamagents exec`: the same backend as the TUI, one headless input (§9).
fn run_exec(args: &Args) -> i32 {
    let Some(prompt) = args.positional.clone() else {
        eprintln!("exec 需要提示词：teamagents exec [--json] [--timeout SEC] \"…\"");
        return 2;
    };
    let state_root = args.state_root.clone().map(PathBuf::from).unwrap_or_else(teamagents_engine::v2_root);
    let socket = match ensure_daemon(&state_root, args.model.clone()) {
        Ok(socket) => socket,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    teamagents_engine::v2::exec::run(teamagents_engine::v2::exec::ExecOptions {
        socket,
        prompt,
        timeout_s: args.timeout.unwrap_or(900),
        json_out: args.exec_json,
    })
}

/// Start the session daemon when the socket is not live, then return the socket
/// path. Detached on purpose: the session must outlive this client (§9).
fn ensure_daemon(state_root: &Path, model: Option<String>) -> Result<PathBuf, String> {
    let socket = state_root.join("daemon.sock");
    // liveness is a *connection*, not the presence of a socket file: a crashed
    // daemon leaves a stale file that would make bind fail if we kept it
    if socket.exists() {
        if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            return Ok(socket);
        }
        let _ = std::fs::remove_file(&socket);
    }
    let exe = std::env::current_exe().map_err(|e| format!("current exe: {e}"))?;
    let model = model.or_else(default_model_key);
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
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    command.spawn().map_err(|e| format!("无法启动 daemon: {e}"))?;
    for _ in 0..150 {
        if socket.exists() {
            return Ok(socket);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Err(format!("daemon 未在 {} 处就绪；请手动运行 teamagents daemon 查看原因", socket.display()))
}

/// The leader's model key when the caller did not choose one: the documented
/// default, else the only configured key.
fn default_model_key() -> Option<String> {
    let catalog = teamagents_engine::config::load_user_config(&teamagents_engine::config::user_config_path()).ok()?;
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
        Some("daemon") => cli::daemon(args.state_root.clone(), args.cwd.clone(), args.model.clone(), args.full_auto),
        Some("init") => cli::init(args.state_root.clone().map(PathBuf::from)),
        Some("doctor") => cli::doctor(args.state_root.clone().map(PathBuf::from)),
        Some("version") => cli::version(),
        Some("exec") => run_exec(&args),
        // v1-only entry points stayed behind with the retired backend (§14/R29)
        Some("validate") | Some("sessions") | Some("serve") | Some("repl") => {
            eprintln!(
                "teamagents {}：旧后端已随 R2 重构退役；团队由 Leader 通过 spawn/delegate 建立，会话由 daemon 拥有。\n用 teamagents 进入 TUI，或用 teamagents exec \"…\" 跑一次无头输入。",
                args.command.as_deref().unwrap_or("")
            );
            2
        }
        _ if args.plain || args.resume.is_some() || args.team.is_some() => {
            eprintln!("--plain/--resume/--team 随旧后端退役；用 teamagents（v2 TUI）或 teamagents exec。");
            2
        }
        _ => run_tui(&args),
    };
    if args.command.is_none() && std::env::var("TEAMAGENTS_ENGINE").is_err() && !args.plain {
        // only reachable for a direct CLI run; nothing to do
    }
    let _ = VERSION;
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
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
