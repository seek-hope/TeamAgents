//! teamagents: single Rust entry point — CLI, TUI launcher, and the headless
//! session worker the TUI talks to (`serve`).

use std::path::{Path, PathBuf};
use teamagents_engine::{cli, tools, worker, VERSION};

fn usage() -> ! {
    eprintln!("teamagents [--cwd DIR] [--resume ID] [--full-auto] [--team SPEC.json] [--plain]");
    eprintln!("  teamagents doctor | validate SPEC | sessions [-v] [prune --days N [--dry-run]] | version");
    eprintln!("  teamagents exec --json [--timeout SEC] [--check COMMAND] PROMPT|- ");
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
    pub timeout: Option<u64>,
    pub checks: Vec<String>,
    pub exec_json: bool,
    pub dry_run: bool,
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
        timeout: None,
        checks: Vec::new(),
        exec_json: false,
        dry_run: false,
    };
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--cwd" => {
                if args.cwd.is_some() { usage(); }
                let v = argv.get(i + 1).cloned().filter(|v| !v.starts_with('-')).unwrap_or_else(|| usage()); args.cwd = Some(v);
                i += 2;
            }
            "--resume" => {
                if args.resume.is_some() { usage(); }
                let v = argv.get(i + 1).cloned().filter(|v| !v.starts_with('-')).unwrap_or_else(|| usage()); args.resume = Some(v);
                i += 2;
            }
            "--team" => {
                if args.team.is_some() { usage(); }
                let v = argv.get(i + 1).cloned().filter(|v| !v.starts_with('-')).unwrap_or_else(|| usage()); args.team = Some(v);
                i += 2;
            }
            "--full-auto" => {
                if args.full_auto { usage(); }
                args.full_auto = true;
                i += 1;
            }
            "--plain" => {
                if args.plain { usage(); }
                args.plain = true;
                i += 1;
            }
            "-v" | "--verbose" => {
                if args.verbose { usage(); }
                args.verbose = true;
                i += 1;
            }
            "serve" | "doctor" | "validate" | "sessions" | "version" | "exec" => {
                if args.command.is_some() { usage(); }
                args.command = Some(argv[i].clone());
                if argv[i] == "exec" { args.exec_json = false; }
                i += 1;
            }
            "--json" if args.command.as_deref() == Some("exec") => { if args.exec_json { usage(); } args.exec_json = true; i += 1; }
            "--timeout" if args.command.as_deref() == Some("exec") => {
                if args.timeout.is_some() { usage(); }
                let raw = argv.get(i + 1).cloned().filter(|v| !v.starts_with('-')).unwrap_or_else(|| usage());
                let parsed = raw.parse::<u64>().unwrap_or_else(|_| usage());
                if parsed == 0 { usage(); }
                args.timeout = Some(parsed); i += 2;
            }
            "--check" if args.command.as_deref() == Some("exec") => {
                args.checks.push(argv.get(i + 1).cloned().filter(|v| !v.starts_with('-')).unwrap_or_else(|| usage())); i += 2;
            }
            "--days" if args.command.as_deref() == Some("sessions") => {
                if args.timeout.is_some() { usage(); }
                let raw = argv.get(i + 1).cloned().filter(|v| !v.starts_with('-')).unwrap_or_else(|| usage());
                args.timeout = Some(raw.parse::<u64>().unwrap_or_else(|_| usage()));
                i += 2;
            }
            "--dry-run" if args.command.as_deref() == Some("sessions") => {
                if args.dry_run { usage(); }
                args.dry_run = true;
                i += 1;
            }
            other if !other.starts_with('-') || other == "-" => {
                if args.positional.is_some() { usage(); }
                args.positional = Some(argv[i].clone());
                i += 1;
            }
            _ => usage(),
        }
    }
    if args.command.as_deref() == Some("exec") && !args.exec_json { usage(); }
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
        Some("sessions") => match args.positional.as_deref() {
            Some("prune") => cli::prune_sessions_cmd(args.timeout.unwrap_or(30), args.dry_run),
            _ => cli::list_sessions_cmd(args.verbose),
        },
        Some("version") => cli::version(),
        Some("exec") => cli::exec_json(&cli::ExecOptions { cwd: args.cwd.clone(), resume: args.resume.clone(), full_auto: args.full_auto, team: args.team.clone(), timeout: args.timeout, checks: args.checks.clone(), prompt: args.positional.clone() }),
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
