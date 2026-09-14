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
        assert_eq!(roots, vec![
            std::path::PathBuf::from("/usr/local/bin/teamagents"),
            std::path::PathBuf::from("/usr/local/bin"),
            std::path::PathBuf::from("/usr/local"),
            std::path::PathBuf::from("/usr"),
            std::path::PathBuf::from("/"),
        ]);
        assert!(!roots.contains(&cwd), "cwd must not be a search root for teamagents-tui");
    }
}
