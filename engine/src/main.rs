//! teamagents: single Rust entry point — CLI, TUI launcher, and the headless
//! session worker the TUI talks to (`serve`).

use std::path::{Path, PathBuf};
use teamagents_engine::{cli, tools, worker, VERSION};

fn usage() -> ! {
    eprintln!("teamagents [--cwd DIR] [--resume ID] [--full-auto] [--team SPEC.json] [--plain]");
    eprintln!("  teamagents doctor | validate SPEC | sessions [-v] | version");
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
}

fn parse_args() -> Args {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut args = Args {
        cwd: None,
        resume: None,
        full_auto: false,
        team: None,
        plain: false,
        verbose: false,
        command: None,
        positional: None,
    };
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--cwd" => {
                args.cwd = argv.get(i + 1).cloned();
                i += 2;
            }
            "--resume" => {
                args.resume = argv.get(i + 1).cloned();
                i += 2;
            }
            "--team" => {
                args.team = argv.get(i + 1).cloned();
                i += 2;
            }
            "--full-auto" => {
                args.full_auto = true;
                i += 1;
            }
            "--plain" => {
                args.plain = true;
                i += 1;
            }
            "-v" | "--verbose" => {
                args.verbose = true;
                i += 1;
            }
            "serve" | "doctor" | "validate" | "sessions" | "version" => {
                args.command = Some(argv[i].clone());
                i += 1;
            }
            other if !other.starts_with('-') => {
                args.positional = Some(argv[i].clone());
                i += 1;
            }
            _ => usage(),
        }
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
    let mut roots: Vec<PathBuf> = vec![];
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }
    if let Some(exe) = &exe {
        roots.extend(exe.ancestors().map(Path::to_path_buf));
    }
    for root in roots {
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

fn run_tui(args: &Args) -> i32 {
    let Some(binary) = find_tui_binary() else {
        eprintln!("找不到 teamagents-tui（先 `cd tui && cargo build`，或用 TEAMAGENTS_TUI 指定路径）");
        return 1;
    };
    let mut command = std::process::Command::new(binary);
    if let Some(cwd) = &args.cwd {
        command.args(["--cwd", cwd]);
    }
    if let Some(resume) = &args.resume {
        command.args(["--resume", resume]);
    }
    if args.full_auto {
        command.arg("--full-auto");
    }
    if let Some(team) = &args.team {
        command.args(["--team", team]);
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

fn main() {
    let args = parse_args();
    let code = match args.command.as_deref() {
        Some("serve") => worker::serve(),
        Some("doctor") => cli::doctor(),
        Some("validate") => match &args.positional {
            Some(path) => cli::validate_spec(path),
            None => usage(),
        },
        Some("sessions") => cli::list_sessions_cmd(args.verbose),
        Some("version") => cli::version(),
        _ if args.plain => cli::repl(args.cwd.clone(), args.resume.clone(), args.full_auto, args.team.clone()),
        _ => run_tui(&args),
    };
    if args.command.is_none() && std::env::var("TEAMAGENTS_ENGINE").is_err() && !args.plain {
        // only reachable for a direct CLI run; nothing to do
    }
    let _ = VERSION;
    std::process::exit(code);
}
