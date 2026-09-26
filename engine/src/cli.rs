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
                    format!("{} (journal_mode={journal}, synchronous={sync})", v2_root.display()),
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
        // Skills and instruction files come from configured paths, and a path that
        // does not resolve is otherwise silent: `skill` answers "no skills
        // configured" at tool time and a missing instruction file simply never
        // reaches a prompt. The shipped config registers `~/.agents/skills` (D-34),
        // so the row also tells a fresh install whether that root is really there.
        let roots: Vec<(&String, PathBuf)> =
            catalog.skills_paths.iter().map(|raw| (raw, crate::config::expand_home(raw))).collect();
        let missing: Vec<&str> = roots.iter().filter(|(_, path)| !path.is_dir()).map(|(raw, _)| raw.as_str()).collect();
        let skills: usize = roots
            .iter()
            .filter(|(_, path)| path.is_dir())
            .map(|(_, path)| crate::tools::skill_candidates(path).len())
            .sum();
        optional_check(
            &mut results,
            "skills",
            roots.iter().any(|(_, path)| path.is_dir()) && missing.is_empty(),
            match (roots.len(), missing.len()) {
                (0, _) => "none configured: skills_paths in the user config registers a root (the shipped config uses ~/.agents/skills)".into(),
                (_, missing_count) if missing_count > 0 => format!(
                    "a configured root does not exist and is ignored, so those skills never load: {}",
                    missing.join(", ")
                ),
                (count, _) => format!("{skills} skill(s) under {count} configured root(s)"),
            },
        );
        // D-102 (D-75's rule): nothing in this build reads these files — a member's system text comes from its
        // own profile — so the row says "declared, not applied" instead of promising they reach a prompt. It
        // still names a path that does not resolve, because that is a config mistake either way.
        if !catalog.instruction_files.is_empty() {
            let missing: Vec<&str> = catalog
                .instruction_files
                .iter()
                .filter(|raw| !crate::config::expand_home(raw).is_file())
                .map(String::as_str)
                .collect();
            let mut detail = format!(
                "{} declared, not applied: this release does not read instruction files into a prompt \
                 (a member's instructions come from its own profile)",
                catalog.instruction_files.len()
            );
            if !missing.is_empty() {
                detail.push_str(&format!("; missing: {}", missing.join(", ")));
            }
            optional_check(&mut results, "instruction files", false, detail);
        }
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
        // A declared MCP service is bound to every member at boot (D-74), so a
        // mistyped command stops the session there: name it here first, where the
        // user can still fix it without reading a daemon log.
        for (name, binding) in &catalog.tools {
            if binding.kind != "mcp" {
                continue;
            }
            let label = format!("tools.{name}");
            let transport = binding.mcp_transport.as_deref().unwrap_or("stdio");
            match transport {
                "http" => optional_check(
                    &mut results,
                    &label,
                    binding.url.as_deref().is_some_and(|url| url.starts_with("http")),
                    match binding.url.as_deref() {
                        Some(url) => format!("http transport at {url}"),
                        None => "kind = \"mcp\" with mcp_transport = \"http\" needs a url".into(),
                    },
                ),
                "stdio" => match binding.command.clone() {
                    Some(command) if command.contains("${") => optional_check(
                        &mut results,
                        &label,
                        true,
                        format!("command {command:?} resolves an environment reference at start"),
                    ),
                    Some(command) => {
                        let path = Path::new(&command);
                        let runnable = if path.components().count() > 1 {
                            path.is_file() && is_executable(path)
                        } else {
                            which(&command).is_some()
                        };
                        optional_check(
                            &mut results,
                            &label,
                            runnable,
                            format!(
                                "{command:?} {} (bound at start{})",
                                if runnable { "is runnable" } else { "is not runnable, so the member fails to start" },
                                if binding.required { ", required" } else { ", optional" }
                            ),
                        )
                    }
                    None => {
                        optional_check(&mut results, &label, false, "kind = \"mcp\" over stdio needs a command".into())
                    }
                },
                other => optional_check(
                    &mut results,
                    &label,
                    false,
                    format!("mcp_transport {other:?} is not one this build speaks (stdio, http)"),
                ),
            }
        }
        // The web half of the same section (D-78): the executor resolves these lazily
        // at the first call, so a typo or an unset credential otherwise shows up only
        // in a tool receipt. Name each declared binding and whether its credential is
        // there; a binding the session could never offer is a FAIL.
        // A config that declares none is worth a row of its own: the README advertises
        // "web search and fetch", and without a `[tools.*]` entry the model is offered
        // neither, which a fresh session would otherwise never say (D-79).
        if !catalog.tools.values().any(|binding| matches!(binding.kind.as_str(), "web_search" | "web_fetch")) {
            optional_check(
                &mut results,
                "web tools",
                false,
                "none configured: the model is offered neither web_search nor web_fetch. Fetching needs no \
                 credential ([tools.fetch] with kind = \"web_fetch\"); web_search also wants \
                 provider/url/api_key_env"
                    .into(),
            );
        }
        for (name, binding) in &catalog.tools {
            if !matches!(binding.kind.as_str(), "web_search" | "web_fetch") {
                continue;
            }
            let provider = binding.provider.as_deref().unwrap_or("anysearch");
            let credential = binding.api_key_env.as_deref();
            let present = credential.map(|key| std::env::var(key).is_ok());
            // the search provider is chosen by name; a name this build does not speak
            // is reported on the binding's own row as well as in the verdict below
            let supported = binding.kind != "web_search" || provider == "anysearch";
            optional_check(
                &mut results,
                &format!("tools.{name}"),
                present.unwrap_or(true) && supported,
                match (credential, present) {
                    _ if !supported => format!(
                        "{} provider {provider:?} is not one this build speaks (anysearch)",
                        binding.kind
                    ),
                    (Some(key), Some(false)) => format!(
                        "{} via {provider:?} needs {key}, which is unset: the tool reports a capability state instead of failing the session",
                        binding.kind
                    ),
                    (Some(key), _) => format!("{} via {provider:?}, credential {key} is set", binding.kind),
                    (None, _) => format!("{} via {provider:?}, no api_key_env configured", binding.kind),
                },
            );
        }
        if let Err(error) = crate::tools::web_tools(catalog, &default_bindings()) {
            check(&mut results, "web tools", false, error);
        }
        // a session with neither ceiling runs until the user stops it, so the
        // user should see what (if anything) bounds their goals
        let tokens = catalog.limits.max_total_tokens;
        let minutes = catalog.limits.deadline_minutes;
        optional_check(
            &mut results,
            "goal limits",
            tokens.is_some() || minutes.is_some(),
            match (tokens, minutes) {
                (Some(tokens), Some(minutes)) => format!("max_total_tokens={tokens}, deadline_minutes={minutes}"),
                (Some(tokens), None) => format!("max_total_tokens={tokens} (no deadline)"),
                (None, Some(minutes)) => format!("deadline_minutes={minutes} (no usage ceiling)"),
                (None, None) => "none: a goal (and the session) runs until you stop it or the budget is reached; [limits] in the user config adds a ceiling".into(),
            },
        );
        // a configured check runs unattended at the completion boundary, so the
        // user should see exactly which commands will gate their goals
        optional_check(
            &mut results,
            "completion checks",
            !catalog.checks.is_empty(),
            if catalog.checks.is_empty() {
                "none configured: a goal settles on the model's own report ([[checks]] in the user config adds machine-checked acceptance)".into()
            } else {
                let ids: Vec<&str> = catalog.checks.iter().map(|check| check.id.as_str()).collect();
                format!("{} configured and run at the completion boundary: {}", catalog.checks.len(), ids.join(", "))
            },
        );
        // D-75: `[retention]` is accepted (it stays user-config-only, like hooks and
        // checks) but nothing in this release archives or prunes a session, so the row
        // says that instead of reporting the numbers as if they were in effect —
        // deleting history is a destructive feature that needs the user's word.
        if catalog.retention.archived_days > 0 || catalog.retention.history_days > 0 {
            optional_check(
                &mut results,
                "retention",
                false,
                format!(
                    "archived_days={} history_days={} are not applied: this release never archives or prunes a \
                     session, so nothing is deleted (the keys are accepted, and kept user-config-only, for the \
                     session layout of earlier releases)",
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

/// The capabilities a session binds (D-78): the daemon boots with this list and
/// `doctor` reports the surface through the same one.
fn default_bindings() -> Vec<String> {
    crate::bound::DEFAULT_BINDINGS.iter().map(|name| (*name).to_string()).collect()
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
        // an empty catalog is a first run, not a missing flag: name the step that
        // creates one instead of leaving the user with "(available: )" (D-73)
        None if available.is_empty() => {
            return Err(format!(
                "the user config has no model profile: run `teamagents init` to write {} (then `teamagents doctor`)",
                user_config_path().display()
            ))
        }
        None => return Err(format!("use --model to name a catalog profile (available: {})", available.join(", "))),
    };
    if !available.contains(&model_key) {
        return Err(format!(
            "model {model_key:?} is not in the user catalog (available: {}); `teamagents doctor` reports what {} resolves to",
            available.join(", "),
            user_config_path().display()
        ));
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
            // The mode is the user's decision and there are two places to make it:
            // the flag asks for host execution *now*, and `[permissions] mode` in the
            // user's own config sets the session default (a project file never can —
            // `permission_mode_from_config` reads the user config only). D-75: the
            // config key was parsed, validated and then ignored, so a user who wrote
            // `mode = "full_auto"` silently ran in approved_scope.
            permissions: if full_auto { "full_auto".into() } else { crate::config::permission_mode_from_config()? },
            catalog,
            bindings: default_bindings(),
            max_retries: 2,
            storage_queue: 256,
            poll: Duration::from_millis(100),
            // the user's acceptance checks gate every goal this session runs
            goal_limits: crate::config::goal_limits(&catalog_for_factory)?,
            require_shell_approval: !full_auto,
            provider_factory: move |id: &str, profile: &KernelProfile| {
                // Never panic here: this runs inside the supervisor's discovery
                // loop, where a panic stops every instance from being driven
                // (D-59). An instance whose model cannot be built gets a provider
                // that fails its own requests permanently, so it parks through
                // the ordinary classified path and the session lives on.
                match crate::providers::build_for_model(&catalog_for_factory, &profile.model) {
                    Ok(provider) => provider,
                    Err(reason) => {
                        eprintln!("daemon: instance {id} cannot be driven: {reason}");
                        crate::providers::AnyProvider::Unavailable { reason }
                    }
                }
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
