//! CLI entry points: init / doctor / daemon / exec / version.

use crate::config::{load_user_config, missing_key_envs, sessions_dir, user_config_path};
use crate::tools::{bwrap_available, shell_run, which};
use crate::VERSION;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub fn init(state_root: Option<PathBuf>) -> i32 {
    let path = user_config_path();
    match crate::config::initialize_config(&path) {
        Ok(created) => {
            if created {
                println!("wrote config: {}", path.display());
                println!("default model: deepseek-flash (context 1,000,000; reasoning effort max).");
                println!("export DEEPSEEK_API_KEY in this terminal; edit the config above for other services.");
            } else {
                println!("kept the existing config: {} (not overwritten)", path.display());
                println!("set the credential environment variable named by api_key_env in that config.");
            }
            if let Err(error) = prepare_v2_root(state_root) {
                eprintln!("could not prepare the state root: {error}");
                return 1;
            }
            println!("next: teamagents doctor, then run teamagents in your project directory.");
            0
        }
        Err(error) => {
            eprintln!("init failed: {error}");
            1
        }
    }
}

/// Prepare (or verify) the v2 session state root: an empty directory gets the
/// format stamp through the store, a v2 root is verified, and anything else is
/// refused — never reinterpreted (A34, §4.4).
pub fn prepare_v2_root(state_root: Option<PathBuf>) -> Result<PathBuf, String> {
    let root = state_root.unwrap_or_else(crate::v2_root);
    std::fs::create_dir_all(&root).map_err(|e| format!("create {}: {e}", root.display()))?;
    let db = root.join("session.sqlite");
    // opening with create stamps format/schema; opening an existing foreign or
    // older database fails loudly here instead of mid-session
    teamagents_core::v2::Control::open(&db, "doctor", true).map_err(|e| format!("{}: {e}", db.display()))?;
    println!("state root ready: {}", root.display());
    println!("  session db: {}", db.display());
    println!("  socket:     {}", root.join("daemon.sock").display());
    if let Some(legacy) = legacy_layout_hint() {
        println!("  note: {legacy}");
    }
    Ok(root)
}

/// Legacy v1 state: reported, never touched here (R28 owns cleaning it).
fn legacy_layout_hint() -> Option<String> {
    let sessions = crate::config::sessions_dir();
    if sessions.is_dir() {
        return Some(format!(
            "found an older release's sessions directory at {} (the old format is not migrated; remove it by an explicit inventory — nothing is deleted automatically)",
            sessions.display()
        ));
    }
    None
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata().map(|meta| meta.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

fn check(results: &mut Vec<(String, &'static str, String)>, name: &str, ok: bool, detail: String) {
    results.push((name.to_string(), if ok { "ok  " } else { "FAIL" }, detail));
}

fn optional_check(results: &mut Vec<(String, &'static str, String)>, name: &str, ok: bool, detail: String) {
    results.push((name.to_string(), if ok { "ok  " } else { "WARN" }, detail));
}

pub fn doctor(state_root: Option<PathBuf>) -> i32 {
    let mut results = vec![];
    // the core is a library now: report the linked core's own version string
    check(&mut results, "rust core", true, format!("teamagents-core {}", teamagents_core::core_version()));
    let config_path = user_config_path();
    let catalog = load_user_config(&config_path);
    match &catalog {
        Ok(catalog) => {
            let models: Vec<&String> = catalog.models.keys().collect();
            let tools: Vec<&String> = catalog.tools.keys().collect();
            check(
                &mut results,
                "user config",
                !catalog.models.is_empty(),
                if catalog.models.is_empty() {
                    format!(
                        "{}: no model configured yet; run teamagents init on a fresh install, or add [models.leader_main] to the file",
                        config_path.display()
                    )
                } else {
                    format!("{} models={models:?} tools={tools:?}", config_path.display())
                },
            );
            let mut keys: Vec<_> = missing_key_envs(catalog).into_iter().collect();
            keys.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, present) in keys {
                let profile = &catalog.models[&name];
                let env = profile.api_key_env.clone().unwrap_or_default();
                check(
                    &mut results,
                    &format!("model profile {name}"),
                    present,
                    format!(
                        "{}/{} {}",
                        profile.provider,
                        profile.model,
                        if present {
                            String::new()
                        } else {
                            format!("(environment variable {env} is unset or empty; set it and retry)")
                        }
                    ),
                );
            }
        }
        Err(e) => check(&mut results, "user config", false, e.clone()),
    }
    // R27/A36: the v2 state root must be identifiable and usable; the legacy
    // layout is only reported (its cleanup belongs to §14/R28)
    let v2_root = state_root.unwrap_or_else(crate::v2_root);
    let v2_db = v2_root.join("session.sqlite");
    if !v2_db.exists() {
        optional_check(
            &mut results,
            "v2 state root",
            false,
            format!("not initialized yet ({}); teamagents init or teamagents daemon creates it", v2_root.display()),
        );
    } else {
        match teamagents_core::v2::Control::open(&v2_db, "doctor", false) {
            Ok(control) => {
                let conn = control.connection();
                let journal: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0)).unwrap_or_default();
                // PRAGMA synchronous answers with the numeric level (2 = FULL)
                let sync: i64 = conn.query_row("PRAGMA synchronous", [], |row| row.get(0)).unwrap_or(-1);
                let sync = match sync {
                    0 => "OFF".to_string(),
                    1 => "NORMAL".to_string(),
                    2 => "FULL".to_string(),
                    3 => "EXTRA".to_string(),
                    other => other.to_string(),
                };
                let probe = conn.execute_batch("CREATE TABLE IF NOT EXISTS doctor_probe(x); DROP TABLE doctor_probe;");
                check(
                    &mut results,
                    "v2 state root",
                    journal.eq_ignore_ascii_case("wal") && probe.is_ok(),
                    format!("{}（journal_mode={journal}, synchronous={sync}）", v2_root.display()),
                );
            }
            Err(error) => check(&mut results, "v2 state root", false, error),
        }
    }
    if let Some(hint) = legacy_layout_hint() {
        optional_check(&mut results, "legacy v1 layout", false, hint);
    }
    let bwrap = bwrap_available();
    // not just "is it installed": run a probe so a broken
    // userns/kernel setup is caught here instead of at the first shell call
    let bwrap_probe = bwrap
        && shell_run("test -e /etc/hostname && test ! -e /home", &std::env::temp_dir(), 20, false, None)
            .map(|out| !out.contains("(exit "))
            .unwrap_or(false);
    check(
        &mut results,
        "bubblewrap isolation",
        bwrap_probe,
        if bwrap_probe {
            "isolation probe passed: system files visible, home directory hidden".into()
        } else if bwrap {
            "bwrap is installed but the isolation probe failed; check that the system allows unprivileged user namespaces".into()
        } else {
            "bwrap not found, shell commands cannot run; Debian/Ubuntu: sudo apt install bubblewrap; Fedora: sudo dnf install bubblewrap; Arch: sudo pacman -S bubblewrap".into()
        },
    );
    // hooks are easy to break silently: a wrong path only shows up as a stderr
    // line at event time, so doctor checks the programs exist and are executable
    if let Ok(catalog) = &catalog {
        for (label, argv) in [("hooks.notify", &catalog.hooks.notify), ("hooks.pre_tool", &catalog.hooks.pre_tool)] {
            let Some(program) = argv.first().filter(|p| !p.trim().is_empty()) else { continue };
            let path = Path::new(program);
            let runnable = if path.components().count() > 1 {
                path.is_file() && is_executable(path)
            } else {
                which(program).is_some()
            };
            check(&mut results, label, runnable, format!("{argv:?}"));
        }
        if catalog.retention.archived_days > 0 || catalog.retention.history_days > 0 {
            check(
                &mut results,
                "retention",
                true,
                format!(
                    "archived_days={} history_days={}",
                    catalog.retention.archived_days, catalog.retention.history_days
                ),
            );
        }
    }
    let dir = sessions_dir();
    let probe = dir.join(".doctor-probe");
    let state_ok = std::fs::create_dir_all(&dir).is_ok()
        && std::fs::write(&probe, "ok").is_ok()
        && std::fs::remove_file(&probe).is_ok();
    check(&mut results, "state directory", state_ok, dir.to_string_lossy().into_owned());

    println!("TeamAgents doctor ({VERSION})");
    let mut failed = 0;
    for (name, status, detail) in &results {
        if *status == "FAIL" {
            failed += 1;
        }
        println!("  [{status}] {name:24} {detail}");
    }
    println!("doctor checks the local machine only and never calls a model API; WARN marks optional capabilities.");
    if failed > 0 {
        1
    } else {
        0
    }
}

pub fn version() -> i32 {
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "version": VERSION,
            "core": "teamagents-core",
            "config": user_config_path().to_string_lossy(),
        }))
        .unwrap_or_default()
    );
    0
}

// ---------------------------------------------------------------------------
// R2-P4 R19: session daemon front-end (plan §9)

/// Run the v2 session daemon: one supervised session over a Unix socket.
/// The daemon owns the engine; TUI/exec are thin clients of its protocol.
pub fn daemon(state_root: Option<String>, cwd: Option<String>, model: Option<String>, full_auto: bool) -> i32 {
    daemon_run(state_root, cwd, model, full_auto)
}

fn daemon_run(state_root: Option<String>, cwd: Option<String>, model: Option<String>, full_auto: bool) -> i32 {
    match daemon_boot(state_root, cwd, model, full_auto) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("daemon: {e}");
            1
        }
    }
}

fn daemon_boot(
    state_root: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    full_auto: bool,
) -> Result<(), String> {
    use teamagents_core::kernel::KernelProfile;
    let catalog = load_user_config(&user_config_path())?;
    let mut available: Vec<String> = catalog.models.keys().cloned().collect();
    available.sort();
    let model_key = match model {
        Some(key) => key,
        None if available.len() == 1 => available[0].clone(),
        None => return Err(format!("use --model to name a catalog profile (available: {})", available.join(", "))),
    };
    if !available.contains(&model_key) {
        return Err(format!("model {model_key:?} is not in the user catalog (available: {})", available.join(", ")));
    }
    // preflight: credentials/protocol resolve at boot, not mid-session (§7)
    crate::providers::build_for_model(&catalog, &model_key)?;
    let workspace = match cwd {
        Some(dir) => PathBuf::from(dir),
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    // one stable root (and socket) per user: init/doctor/daemon/TUI must agree
    // on where the session lives, or the default entry cannot find the daemon
    let state_root = state_root.map(PathBuf::from).unwrap_or_else(crate::v2_root);
    let socket = state_root.join("daemon.sock");
    let catalog_for_factory = catalog.clone();
    let config = crate::v2::daemon::DaemonConfig {
        supervisor: crate::v2::supervisor::SupervisorConfig {
            marker: std::marker::PhantomData,
            session_db: state_root.join("session.sqlite"),
            session_id: "s-main".into(),
            leader_id: "i-leader".into(),
            leader_profile: KernelProfile {
                model: model_key,
                instructions: crate::v2::daemon::LEADER_INSTRUCTIONS.into(),
                tools: crate::reference::basic_tool_schemas(true, true),
                options: json!({}),
                context_window: None,
            },
            state_root: state_root.clone(),
            workspace,
            permissions: if full_auto { "full_auto".into() } else { "approved_scope".into() },
            catalog,
            bindings: vec!["files".into(), "shell".into(), "web".into(), "skills".into()],
            max_retries: 2,
            storage_queue: 256,
            poll: Duration::from_millis(100),
            goal_limits: json!({}),
            require_shell_approval: !full_auto,
            provider_factory: move |_id: &str, profile: &KernelProfile| {
                crate::providers::build_for_model(&catalog_for_factory, &profile.model)
                    .unwrap_or_else(|e| panic!("provider for {}: {e}", profile.model))
            },
        },
        socket: socket.clone(),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async move {
        let handle = crate::v2::daemon::serve(config).await?;
        eprintln!(
            "teamagents daemon started\n  socket:    {}\n  state root: {}\n  clients can attach now; Ctrl-C stops the daemon (committed state is kept)",
            socket.display(),
            state_root.display()
        );
        tokio::signal::ctrl_c().await.map_err(|e| e.to_string())?;
        eprintln!("\nstopping...");
        handle.shutdown().await
    })
}
