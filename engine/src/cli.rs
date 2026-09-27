//! CLI entry points: init / doctor / daemon / exec / version.

use crate::config::{
    load_user_config_for, missing_key_envs, project_config_path, sessions_dir, state_dir, user_config_path,
};
use crate::tools::{bwrap_available, which};
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
    require_state_root_dir(&root)?;
    // D-227: a root deep enough that `daemon.sock` crosses Linux's `sun_path` limit cannot hold a session, and
    // `init` is where a user meets that — printing the socket path as if it were usable sent them to a later
    // failure whose OS answer names no fix.
    require_socket_path_fits(&root.join("daemon.sock"))?;
    // D-228: and the paths under the root must have the right kind — a directory named `session.sqlite` or
    // `daemon.sock` (or an ancestor that is a file) fails later with an error that names no fix.
    require_state_paths_kind(&root)?;
    let fresh = !root.exists();
    std::fs::create_dir_all(&root).map_err(|e| state_root_uncreatable(&root, &e))?;
    let db = root.join("session.sqlite");
    // opening with create stamps format/schema; opening an existing foreign or
    // older database fails loudly here instead of mid-session
    teamagents_core::v2::Control::open(&db, "doctor", true).map_err(|e| format!("{}: {e}", db.display()))?;
    println!("state root ready: {}", root.display());
    println!("  session db: {}", db.display());
    println!("  socket:     {}", root.join("daemon.sock").display());
    // D-169: the neighbouring typo of D-166's (one path segment over). A root that does not exist yet is legal
    // and gets created (D-149), so a path *named like the database file* used to produce a directory called
    // `session.sqlite` with a database inside it — measured 2026-09-27. The note names the shape; it never
    // refuses, because a fresh root at any name is what the user asked for.
    if fresh {
        if let Some(hint) = database_shaped_root_hint(&root) {
            println!("  note: {hint}");
        }
    }
    if let Some(legacy) = legacy_layout_hint() {
        println!("  note: {legacy}");
    }
    Ok(root)
}

/// A note when the just-created root's own name looks like the session database.
fn database_shaped_root_hint(root: &Path) -> Option<String> {
    let name = root.file_name()?.to_string_lossy().into_owned();
    (name == "session.sqlite" || name.ends_with(".sqlite")).then(|| {
        format!(
            "{name} looks like a session database, not a state root: the state root is the *directory* that \
             holds session.sqlite and daemon.sock, and a fresh root was created at {} — if you meant that \
             database file, pass its parent directory next time",
            root.display()
        )
    })
}

/// Legacy v1 state: reported, never touched here (R28 owns cleaning it).
///
/// A **non-empty** directory is the evidence. An empty one has nothing to migrate, and this build used to
/// create exactly that: `doctor`'s writability probe ran on this legacy path, so `doctor` followed by `init`
/// told the user to remove "an older release's sessions directory" that `doctor` itself had just made
/// (measured 2026-09-26, D-149).
fn legacy_layout_hint() -> Option<String> {
    let sessions = sessions_dir();
    let holds_sessions = std::fs::read_dir(&sessions).map(|mut entries| entries.next().is_some()).unwrap_or(false);
    if holds_sessions {
        return Some(format!(
            "found an older release's sessions directory at {} (the old format is not migrated; remove it by an explicit inventory — nothing is deleted automatically)",
            sessions.display()
        ));
    }
    None
}

/// `(files, bytes)` under one state root's artifact directories, or `None` when there are none.
///
/// Artifacts live per member (`<state root>/instances/<id>/artifacts`, the driver's own root — D-174) and, in a
/// state root written by an older release, possibly also at `<state root>/artifacts`; both are counted so the
/// report does not depend on which layout produced the files.
fn artifact_footprint(root: &Path) -> Option<(u64, u64)> {
    let mut roots = vec![root.join("artifacts")];
    if let Ok(entries) = std::fs::read_dir(root.join("instances")) {
        roots.extend(entries.flatten().map(|entry| entry.path().join("artifacts")));
    }
    let (mut files, mut bytes) = (0u64, 0u64);
    for dir in roots {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    files += 1;
                    bytes += meta.len();
                }
            }
        }
    }
    (files > 0).then_some((files, bytes))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata().map(|meta| meta.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// One `doctor` row: the name, the status word and the detail line.
type DoctorRow = (String, &'static str, String);
/// A row reporter: `check` for something that has to work, `optional_check` for a capability.
type RowReporter = fn(&mut Vec<DoctorRow>, &str, bool, String);

fn check(results: &mut Vec<DoctorRow>, name: &str, ok: bool, detail: String) {
    results.push((name.to_string(), if ok { "ok  " } else { "FAIL" }, detail));
}

/// Print what the project-config merge decided (D-244).
///
/// The daemon is detached, so its stderr is `<state root>/daemon.log`; a user who never reads that file gets the
/// same facts from `doctor`'s `project config` row, built from the same [`ProjectMerge`].
fn report_project_merge(merge: &crate::config::ProjectMerge) {
    if !merge.present {
        return;
    }
    eprintln!(
        "teamagents: project config {} ({}): accepted {}{}",
        merge.path.display(),
        if merge.trusted { "trusted" } else { "untrusted" },
        if merge.accepted.is_empty() { "nothing".to_string() } else { merge.accepted.join(", ") },
        if merge.refused.is_empty() { String::new() } else { format!("; refused {}", merge.refused.join("; ")) }
    );
}

fn optional_check(results: &mut Vec<DoctorRow>, name: &str, ok: bool, detail: String) {
    results.push((name.to_string(), if ok { "ok  " } else { "WARN" }, detail));
}

/// The `v2 state root` row: the journal mode, the sync level and the SQLite this build **links** — DESIGN §4.4
/// makes that version part of the durability guarantee, so the row fails when it predates the WAL-reset fix
/// (D-183). Split out because the branch that refuses an old library cannot be provoked on a machine that
/// links a new one; its test is in this file.
fn v2_state_root_row(root: &Path, journal: &str, sync: &str, version: &str, carries_the_fix: bool) -> (bool, String) {
    if carries_the_fix {
        (true, format!("{} (journal_mode={journal}, synchronous={sync}, sqlite={version})", root.display()))
    } else {
        (
            false,
            format!(
                "{} (journal_mode={journal}, synchronous={sync}, sqlite={version} lacks the WAL-reset fix {}, \
                 which DESIGN §4.4 requires of the SQLite this build selects)",
                root.display(),
                teamagents_core::v2::store::MIN_SQLITE_VERSION
            ),
        )
    }
}

pub fn doctor(state_root: Option<PathBuf>) -> i32 {
    let mut results = vec![];
    // the core is a library now: report the linked core's own version string
    check(&mut results, "rust core", true, format!("teamagents-core {}", teamagents_core::core_version()));
    let config_path = user_config_path();
    // D-244: doctor reads what a session *started in this directory* would read — the user config plus the
    // repository-local one, under the user's trust gate — and reports the merge's decision in its own row,
    // because the daemon's copy of it lands in `<state root>/daemon.log`.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let catalog = load_user_config_for(&cwd);
    match &catalog {
        Ok((catalog, merge)) => {
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
            if merge.present {
                optional_check(
                    &mut results,
                    "project config",
                    merge.refused.is_empty(),
                    format!(
                        "{} ({}): accepted {}{}",
                        merge.path.display(),
                        if merge.trusted { "trusted" } else { "untrusted" },
                        if merge.accepted.is_empty() { "nothing".to_string() } else { merge.accepted.join(", ") },
                        if merge.refused.is_empty() {
                            String::new()
                        } else {
                            format!("; refused {}", merge.refused.join("; "))
                        }
                    ),
                );
            } else {
                check(
                    &mut results,
                    "project config",
                    true,
                    format!(
                        "none at {} (a session started here reads only {})",
                        merge.path.display(),
                        config_path.display()
                    ),
                );
            }
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
    // layout is only reported (its cleanup belongs to the archived plan's §14/R28)
    let v2_root = state_root.unwrap_or_else(crate::v2_root);
    let v2_db = v2_root.join("session.sqlite");
    // D-227/D-228: the rows that would have been green in the probes that found them — a root whose socket
    // path cannot be bound, and paths under it whose *kind* is wrong, are FAILs: nothing can use that root.
    match require_socket_path_fits(&v2_root.join("daemon.sock")) {
        Ok(()) => {}
        Err(error) => check(&mut results, "daemon socket", false, error),
    }
    // D-228, in D-166's shape: when the paths' kind is wrong, the rows below would report SQLite's raw
    // `unable to open database file` (three times, and no fix), so this row replaces them rather than joining
    // them — and `init` refuses the same root with the same words.
    let mut kind_error = require_state_paths_kind(&v2_root).err();
    if let Some(error) = kind_error.take() {
        check(&mut results, "state root paths", false, error);
    } else if v2_root.exists() && !v2_root.is_dir() {
        // D-166: the root is a *file* (usually the session database itself). The rows below would say "not
        // initialized yet; `teamagents init` creates it" — advice that cannot be followed, because `init`
        // refuses the same path. So this is a FAIL, with the words the other entry points use.
        check(&mut results, "v2 state root", false, state_root_not_a_dir(&v2_root));
    } else if !v2_db.exists() {
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
                let sqlite = rusqlite::version();
                let carries_the_fix = teamagents_core::v2::store::linked_sqlite_carries_the_wal_reset_fix();
                let (fix_ok, detail) = v2_state_root_row(&v2_root, &journal, &sync, sqlite, carries_the_fix);
                check(
                    &mut results,
                    "v2 state root",
                    journal.eq_ignore_ascii_case("wal") && probe.is_ok() && fix_ok,
                    detail,
                );
            }
            Err(error) => check(&mut results, "v2 state root", false, error),
        }
    }
    // D-174 asked this row to say what is really there and what is not done; D-191 implemented the half that
    // was missing, so the row now states what collection *does* (a driver's boot claims unreferenced artifacts,
    // deletes their bytes and collects their rows) and what it still does not (a schedule beyond boot).
    if let Some((files, bytes)) = artifact_footprint(&v2_root) {
        optional_check(
            &mut results,
            "artifacts",
            false,
            format!(
                "{files} file(s), {:.1} MB under {}/instances/*/artifacts; oversized tool output is pruned per \
                 member (512 MB), model responses are kept as evidence, and unreferenced artifacts are \
                 collected when a driver boots (DESIGN §4.4; a schedule beyond that is not implemented)",
                bytes as f64 / 1_048_576.0,
                v2_root.display()
            ),
        );
    }
    if let Some(hint) = legacy_layout_hint() {
        optional_check(&mut results, "legacy v1 layout", false, hint);
    }
    let bwrap = bwrap_available();
    // not just "is it installed": `sandbox_state` runs a probe so a broken userns/kernel setup is caught here
    // instead of at the first shell call, and it carries the reason (the same answer the tests branch on, D-114)
    let sandbox = crate::tools::sandbox_state();
    check(
        &mut results,
        "bubblewrap isolation",
        sandbox.is_ok(),
        match &sandbox {
            Ok(()) => "isolation probe passed: system files visible, home directory hidden".into(),
            Err(reason) if bwrap => {
                // a row is one line, so the machine's own words (bwrap's last line) are what a user needs; the
                // generic sentence above them ("the sandbox failed to start …") only says what they already see
                let own = reason.lines().rfind(|line| !line.trim().is_empty()).unwrap_or(reason);
                format!("bwrap is installed but the isolation probe failed: {own}; check that the system allows unprivileged user namespaces")
            }
            Err(_) => {
                "bwrap not found, shell commands cannot run; Debian/Ubuntu: sudo apt install bubblewrap; Fedora: sudo dnf install bubblewrap; Arch: sudo pacman -S bubblewrap".into()
            }
        },
    );
    // hooks are easy to break silently: a wrong path only shows up as a stderr
    // line at event time, so doctor checks the programs exist and are executable
    if let Ok((catalog, _)) = &catalog {
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
                // D-168: name what the *model* sees. The `skills` binding is product-default, so the `skill`
                // tool stays offered whatever the roots are; with none it can only answer the capability state.
                (0, _) => "none configured: the `skill` tool is still offered and answers `no skills configured`. \
                             `skills_paths` in the user config registers a root (the shipped config uses \
                             ~/.agents/skills)"
                    .into(),
                (_, missing_count) if missing_count > 0 => format!(
                    "a configured root is not a usable directory (missing, or a file) and is ignored, so those \
                     skills never load: {}; the `skill` tool stays offered and answers `no skills configured`",
                    missing.join(", ")
                ),
                (count, _) => format!("{skills} skill(s) under {count} configured root(s)"),
            },
        );
        // D-102 asked for this row to say the truth; since D-246 the truth is that the files *do* reach every
        // member's prompt (the driver composes them into every instance's system text), so the row counts what a
        // session will deliver — bytes and files — and a file it cannot read is a WARN naming it.
        if !catalog.instruction_files.is_empty() {
            let (text, unreadable) = crate::config::instruction_text(&catalog.instruction_files);
            let read = catalog.instruction_files.len() - unreadable.len();
            let mut detail = format!(
                "{read} file(s), {} byte(s) reach every member's prompt (the leader's and every child's system \
                 text, read per turn)",
                text.len()
            );
            if !unreadable.is_empty() {
                detail.push_str(&format!("; cannot read: {}", unreadable.join(", ")));
            }
            optional_check(&mut results, "instruction files", unreadable.is_empty(), detail);
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
        // A declared MCP service is bound to every member at boot (D-74), so a mistyped command stops the
        // session there: name it here first, where the user can still fix it without reading a daemon log.
        // What each row *promises* has to be what the boot does (`engine/src/bound.rs`): a `required = true`
        // service that cannot load fails the driver boot — the instance parks and no member ever runs — while
        // an optional one only drops that capability. D-164 measured both reported as WARN under doctor's
        // "WARN marks optional capabilities" footer with exit 0, so a user was told the session was fine while
        // nothing drove it. The rows below are the checks the boot performs *before* it talks to a service: the
        // command is spawned verbatim (nothing expands `${…}` in a command), the `env` map and the bearer
        // variable are read from this environment, and the transport is one of the two this build speaks.
        // Whether a service *answers* needs starting it, which `doctor` does not do (it never spawns anything).
        for (name, binding) in &catalog.tools {
            if binding.kind != "mcp" {
                continue;
            }
            let label = format!("tools.{name}");
            // a required service that cannot load refuses the boot; an optional one only loses a capability
            let row: RowReporter = if binding.required { check } else { optional_check };
            let lost = if binding.required {
                "the session cannot start a member until it is fixed (the instance parks)"
            } else {
                "that capability is dropped and the session still boots"
            };
            let transport = binding.mcp_transport.as_deref().unwrap_or("stdio");
            match transport {
                "http" => match (binding.url.as_deref(), binding.bearer_token_env_var.as_deref()) {
                    (Some(url), _) if !url.starts_with("http") => row(
                        &mut results,
                        &label,
                        false,
                        format!("http transport url {url:?} is not an http(s) URL, so {lost}"),
                    ),
                    (Some(url), Some(var)) if std::env::var(var).is_err() => row(
                        &mut results,
                        &label,
                        false,
                        format!("http transport at {url} needs bearer token {var}, which is unset, so {lost}"),
                    ),
                    (Some(url), Some(var)) => {
                        row(&mut results, &label, true, format!("http transport at {url}, bearer {var} is set"))
                    }
                    (Some(url), None) => row(&mut results, &label, true, format!("http transport at {url}")),
                    (None, _) => row(
                        &mut results,
                        &label,
                        false,
                        format!("kind = \"mcp\" with mcp_transport = \"http\" needs a url, so {lost}"),
                    ),
                },
                "stdio" => match binding.command.as_deref() {
                    None => row(
                        &mut results,
                        &label,
                        false,
                        format!("kind = \"mcp\" over stdio needs a command, so {lost}"),
                    ),
                    // `${…}` in a *command* is expanded by nothing: the string reaches `exec(2)` literally, so
                    // the service never starts. (The `env` map below is where a reference does resolve.)
                    Some(command) if command.contains("${") => row(
                        &mut results,
                        &label,
                        false,
                        format!(
                            "command {command:?} carries a `${{…}}` reference, which nothing expands in a \
                             command (use an absolute path, or a shell wrapper with the reference in args), so {lost}"
                        ),
                    ),
                    Some(command) => {
                        let path = Path::new(command);
                        let runnable = if path.components().count() > 1 {
                            path.is_file() && is_executable(path)
                        } else {
                            which(command).is_some()
                        };
                        // `bound.rs` reads a `${VAR}` env value from this environment and treats an unset one
                        // as a hard error, so a row that stopped at "the command exists" was green while the
                        // boot refused.
                        let unset: Vec<&str> = binding
                            .env
                            .values()
                            .filter_map(|value| value.strip_prefix("${").and_then(|v| v.strip_suffix('}')))
                            .filter(|var| std::env::var(*var).is_err())
                            .collect();
                        let (ok, detail) = if !runnable {
                            (false, format!("{command:?} is not runnable, so {lost}"))
                        } else if !unset.is_empty() {
                            (
                                false,
                                format!(
                                    "{command:?} is runnable but its env {} {} unset, so {lost}",
                                    unset.join(", "),
                                    if unset.len() == 1 { "is" } else { "are" }
                                ),
                            )
                        } else {
                            (true, format!("{command:?} is runnable (bound at start)"))
                        };
                        row(&mut results, &label, ok, detail);
                    }
                },
                other => row(
                    &mut results,
                    &label,
                    false,
                    format!("mcp_transport {other:?} is not one this build speaks (stdio, http), so {lost}"),
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
            let label = format!("tools.{name}");
            let provider = binding.provider.as_deref().unwrap_or("anysearch");
            // D-167: `web_search` needs a credential — DESIGN §7 calls its absence a capability state — and a
            // *required* binding that cannot work refuses the member's start, so that is a FAIL (D-164's rule,
            // applied to the web half). The credential wording comes from the helper the tool call and the boot
            // use, so the three cannot describe the same binding differently. `web_fetch` is the half that
            // needs no credential at all.
            let row: RowReporter = if binding.required { check } else { optional_check };
            let lost = if binding.required {
                "the session cannot start a member until it is fixed (the instance parks)"
            } else {
                "a call answers with that capability state and the session still boots"
            };
            let key = binding.api_key_env.as_deref();
            let (ok, detail) = if binding.kind == "web_fetch" {
                (
                    true,
                    match key {
                        Some(key) => format!("web_fetch needs no credential (api_key_env {key} is not used)"),
                        None => "web_fetch needs no credential".to_string(),
                    },
                )
            } else if provider != "anysearch" {
                (false, format!("web_search provider {provider:?} is not one this build speaks (anysearch), so {lost}"))
            } else {
                match crate::tools::web_search_credential(key) {
                    Ok(_) => (true, format!("web_search via {provider:?}, credential {} is set", key.unwrap_or(""))),
                    Err(reason) => (false, format!("web_search via {provider:?}: {reason}, so {lost}")),
                }
            };
            row(&mut results, &label, ok, detail);
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
        // D-245: `history_days` is applied now — a session boot drops ordinary history under the guards
        // `verification/tla/V2Retention.tla` pins — and `archived_days` is not: this build keeps one session per
        // state root (A33), so there is no archived-session set to walk. The row says which of the two it is,
        // and names the state root's `EVIDENCE` marker when the user has asked for nothing to be pruned here.
        if catalog.retention.archived_days > 0 || catalog.retention.history_days > 0 {
            let root_here = v2_root.clone();
            let mut detail = if catalog.retention.history_days == 0 {
                "history_days=0 keeps the full history: events are the audit trail".to_string()
            } else if crate::config::state_root_marked_as_evidence(&root_here) {
                format!(
                    "history_days={}: this state root carries the {} marker, so no session here prunes anything",
                    catalog.retention.history_days,
                    crate::config::EVIDENCE_MARKER
                )
            } else {
                format!(
                    "history_days={}: a session boot drops events and applied deliveries older than that, keeping \
                     live references (the log's head, a pending wait's fact, a non-terminal instance's lifecycle) \
                     and evaluation evidence",
                    catalog.retention.history_days
                )
            };
            if catalog.retention.archived_days > 0 {
                detail.push_str(&format!(
                    "; archived_days={} is not applied: this build keeps one session per state root (A33), so \
                     there is no archived-session set to walk",
                    catalog.retention.archived_days
                ));
            }
            optional_check(&mut results, "retention", catalog.retention.archived_days == 0, detail);
        }
    }
    // The probe runs on the state directory this build actually uses, never on the legacy `sessions/` path:
    // a check that creates the directory it then warns about is how `init` came to blame an older release for
    // this build's own empty directory (measured 2026-09-26, D-149).
    let dir = state_dir();
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

/// A `--state-root` must be a directory, or not exist yet (it is then created).
///
/// The two shapes a user confuses are one path segment apart — the root *directory* and the
/// `session.sqlite` file inside it — and every entry point joins `session.sqlite`/`daemon.sock` onto the root.
/// Naming the file produced four different raw errnos, none of them naming the mistake (measured 2026-09-27,
/// D-166): `daemon`/`init` answered `File exists (os error 17)`, a read verb `Not a directory (os error 20)`,
/// and `doctor` printed a WARN telling the user to run `init` — a command that then failed the same way.
pub fn require_state_root_dir(path: &Path) -> Result<(), String> {
    if !path.exists() || path.is_dir() {
        return Ok(());
    }
    Err(state_root_not_a_dir(path))
}

/// Linux's `sun_path` is 108 bytes *including* the terminating NUL, so an AF_UNIX path binds only while it is
/// **shorter than that**: measured 2026-09-27, a 107-byte path binds and a 108-byte one fails with
/// `AF_UNIX path too long`.
pub const SOCKET_PATH_LIMIT: usize = 108;

/// A state root deep enough puts the daemon's socket past that limit, and then nothing under it works: the
/// daemon's `bind` fails, and every client's `connect` fails on the same path. The raw answers are the OS's —
/// `bind …: path must be shorter than SUN_LEN` (the daemon) or `AF_UNIX path too long` (a client) — and neither
/// names the fix; the callers that refuse it and the one that reports it (`doctor`) share this wording, so a
/// row and a refusal cannot drift apart (D-227).
pub fn require_socket_path_fits(socket: &Path) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = socket.as_os_str().as_bytes().len();
    if bytes < SOCKET_PATH_LIMIT {
        return Ok(());
    }
    Err(format!(
        "the daemon socket path is {bytes} bytes, at or above Linux's {SOCKET_PATH_LIMIT}-byte `sun_path` \
         limit: {} — point XDG_STATE_HOME (or HOME) at a shorter directory and run `teamagents init` again",
        socket.display()
    ))
}

/// The paths under a state root must have the right *kind* before anything uses them (D-228).
///
/// D-166 fixed the state root that is a *file*; these are the two inversions one segment in, both measured
/// 2026-09-27. A **directory** named `session.sqlite` made `init` fail with SQLite's own
/// `unable to open database file` — the path printed three times and no fix — and a **directory** named
/// `daemon.sock` made the client answer `connect …: Connection refused … start teamagents daemon first`, a
/// diagnosis pointing at the daemon, while `doctor` called that state root `[ok]`. The create path below it is
/// the third shape: an *ancestor* that is a file makes `create_dir_all` answer `Not a directory (os error 20)`,
/// which names neither the component nor the fix.
pub fn require_state_paths_kind(root: &Path) -> Result<(), String> {
    // 1. an ancestor that is a file: the root cannot be created under it
    let mut ancestor = root;
    loop {
        match ancestor.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => ancestor = parent,
            _ => break,
        }
        if ancestor.exists() && !ancestor.is_dir() {
            return Err(format!(
                "{} is a file, so the state root {} cannot be created under it — point --state-root (or \
                 XDG_STATE_HOME) at a directory that exists, or one whose parents do",
                ancestor.display(),
                root.display()
            ));
        }
    }
    // 2. the database path as a directory: it must be the file that holds the session. (Directories only: an
    // *existing* `session.sqlite` is a regular file, and a live socket under the same rule is not a file either —
    // measured while this check first ran, when `!is_file()` refused a legitimate `daemon.sock` and broke three
    // tests whose daemons were running.)
    let db = root.join("session.sqlite");
    if db.is_dir() {
        return Err(format!(
            "{} is a directory, but that path is the session database file — remove the directory (or point \
             --state-root/XDG_STATE_HOME at another one) and run `teamagents init` again",
            db.display()
        ));
    }
    // 3. the socket path as a directory: nothing can bind it
    let socket = root.join("daemon.sock");
    if socket.is_dir() {
        return Err(format!(
            "{} is a directory, but that path is the daemon's socket — remove the directory and start the \
             session again",
            socket.display()
        ));
    }
    // 4. the log the client redirects the daemon into (D-241): the same shape one path further out, and one
    // nothing looked at — `exec` answered a bare `cannot open …/daemon.log: Is a directory (os error 21)`.
    let log = root.join("daemon.log");
    if log.is_dir() {
        return Err(format!(
            "{} is a directory, but that path is where the daemon's log goes — remove the directory (or point \
             --state-root/XDG_STATE_HOME at another one) and start the session again",
            log.display()
        ));
    }
    Ok(())
}

/// The one wording for a state root that cannot be *created* (D-241).
///
/// Five places create one — `init`'s prepare step, the daemon's boot, each instance driver's own root under it,
/// and the client, which only creates it so the daemon's log file has somewhere to land. They answered three
/// different ways and none of them named the flag or a fix: measured 2026-09-27 with the root under a symlink
/// loop, `init` said `could not prepare the state root: create … (os error 40)`, `daemon` said `state root: … (os
/// error 40)` and `exec` said `cannot create … (os error 40)` — while `doctor`'s `[WARN] v2 state root   not
/// initialized yet; teamagents init …` sent the user back to the command that had just failed. They share this
/// wording now, so a refusal cannot drift from the others (D-166's rule, D-227/D-228's shape); `path` may be a
/// directory *under* the state root, which needs the same fix.
pub fn state_root_uncreatable(path: &Path, error: &std::io::Error) -> String {
    format!(
        "the state root {} cannot be created: {error} — point --state-root (or XDG_STATE_HOME) at a directory \
         you can write, or one whose parents do",
        path.display()
    )
}

/// The one wording for the log file the client redirects the daemon into (D-243).
///
/// The client creates `<state root>/daemon.log` so a *detached* daemon has somewhere to complain, and it
/// answered the OS when the file was there but not writable: measured 2026-09-27 with a mode-`000` file,
/// `exec` printed `cannot open …/daemon.log: Permission denied (os error 13)` — no lever, and the fix (that
/// file) only implied. A run that once went through `sudo` leaves exactly that file behind, root-owned.
pub fn daemon_log_unopenable(path: &Path, error: &std::io::Error) -> String {
    format!(
        "the daemon's log {} cannot be opened: {error} — the client writes the daemon's output there, so make it \
         writable or remove it, or start the client with --state-root (or XDG_STATE_HOME) on another directory",
        path.display()
    )
}

/// The one wording for a directory the session needs *under* its state root that cannot be created (D-243).
///
/// Unlike the state root itself (D-241), the user never typed these paths — `<state root>/artifacts`, its
/// `locks`, an isolated shell's `shell` — and their failures named neither the path nor the lever: measured
/// 2026-09-27 with a *file* where the artifact directory goes, the tool answered `cannot create output
/// artifact directory: File exists (os error 17)`, which leaves several candidate directories and nothing to
/// act on (`No space left on device` is the other way in).
pub fn derived_dir_uncreatable(path: &Path, error: &std::io::Error) -> String {
    format!(
        "{} cannot be created: {error} — it is a directory the session needs under its state root; free space \
         there, or start the session with --state-root (or XDG_STATE_HOME) on another directory",
        path.display()
    )
}

/// The one wording for a `--state-root` that cannot be a state root. The callers that refuse it and the one
/// that *reports* it (`doctor`) share this, so a row and a refusal cannot drift apart.
fn state_root_not_a_dir(path: &Path) -> String {
    format!(
        "--state-root {} is not a directory: the state root is the directory that holds session.sqlite and \
         daemon.sock. If you meant the database file, pass the directory that holds it; otherwise name an \
         existing directory, or one that does not exist yet.",
        path.display()
    )
}

/// The workspace a session works in must exist before it boots.
///
/// Every file tool and shell command is confined to this root, and `tools.rs` resolves it with
/// `canonicalize`, so a `--cwd` that is not a directory left the session running against a root nothing
/// could resolve: the model saw a bare `No such file or directory (os error 2)` that never named the flag,
/// while the socket's greeting and the `--json` report named the bad path as `session_workspace` as if it
/// were fine. A path naming a *file* is the same mistake one step further. Measured 2026-09-27 with
/// `exec --cwd <missing>` and `--cwd <afile>` (both exited 0 with the bad path as the workspace, D-163).
pub fn require_workspace_dir(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        return Ok(());
    }
    Err(format!(
        "--cwd {} is not a directory: the session works in it and confines every file and shell command to \
         it, so it has to exist before the session boots. Create it first, point --cwd at another existing \
         directory, or leave --cwd out to work in the current directory",
        path.display()
    ))
}

fn daemon_boot(
    state_root: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    full_auto: bool,
) -> Result<(), String> {
    use teamagents_core::kernel::KernelProfile;
    // D-244: the workspace is resolved *first*, because the repository-local config belongs to the directory the
    // session works in — and the merge's decision goes into the daemon's own log (a detached daemon has no
    // terminal), which `doctor` shows in the same words for a user who never reads that file.
    let workspace = match cwd {
        Some(dir) => PathBuf::from(dir),
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    require_workspace_dir(&workspace)?;
    let (catalog, project) = load_user_config_for(&workspace)?;
    report_project_merge(&project);
    let mut available: Vec<String> = catalog.models.keys().cloned().collect();
    available.sort();
    let model_key = match model {
        Some(key) => key,
        None if available.len() == 1 => available[0].clone(),
        // an empty catalog is a first run, not a missing flag: name the step that
        // creates one instead of leaving the user with "(available: )" (D-73)
        None if available.is_empty() => {
            return Err(format!(
                "no model profile is configured: run `teamagents init` to write {} (then `teamagents doctor`)",
                user_config_path().display()
            ))
        }
        None => return Err(format!("use --model to name a catalog profile (available: {})", available.join(", "))),
    };
    if !available.contains(&model_key) {
        return Err(format!(
            "model {model_key:?} is not in the catalog (available: {}); `teamagents doctor` reports what {} resolves \
             to, and a repository-local {} needs [permissions] trust_project = true",
            available.join(", "),
            user_config_path().display(),
            project_config_path(&workspace).display()
        ));
    }
    // preflight: credentials/protocol resolve at boot, not mid-session (§7)
    crate::providers::build_for_model(&catalog, &model_key)?;
    // one stable root (and socket) per user: init/doctor/daemon/TUI must agree
    // on where the session lives, or the default entry cannot find the daemon
    let state_root = state_root.map(PathBuf::from).unwrap_or_else(crate::v2_root);
    require_state_root_dir(&state_root)?;
    let socket = state_root.join("daemon.sock");
    let catalog_for_factory = catalog.clone();
    // D-168: the members' tool surface follows the config — a web tool is offered only for the kind the catalog
    // declares a binding of (§5.2: binding is the authorization), which is what `doctor`'s row for a config
    // without one already claimed.
    let member_tools = crate::reference::session_tool_schemas(&catalog);
    let config = crate::v2::daemon::DaemonConfig {
        supervisor: crate::v2::supervisor::SupervisorConfig {
            marker: std::marker::PhantomData,
            session_db: state_root.join("session.sqlite"),
            session_id: "s-main".into(),
            leader_id: "i-leader".into(),
            leader_profile: KernelProfile {
                model: model_key,
                instructions: crate::v2::daemon::LEADER_INSTRUCTIONS.into(),
                tools: member_tools,
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
            "teamagents daemon started\n  socket:    {}\n  state root: {}\n  clients can attach now; Ctrl-C or SIGTERM stops it (committed state is kept)",
            socket.display(),
            state_root.display()
        );
        // Ctrl-C is only reachable where the daemon has a terminal, and the daemon users actually have is the
        // detached one (`teamagents`/`exec` start it, §1). SIGTERM is what a user's `kill` and a service
        // manager send, so it must reach the same shutdown: without this the only stop available to a user was
        // an abrupt death that skipped the designed shutdown and left the socket behind (measured 2026-09-26,
        // D-150).
        let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .map_err(|e| e.to_string())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => { result.map_err(|e| e.to_string())?; }
            _ = terminate.recv() => {}
        }
        eprintln!("\nstopping...");
        handle.shutdown().await
    })
}

#[cfg(test)]
mod tests {
    use super::{require_socket_path_fits, v2_state_root_row, SOCKET_PATH_LIMIT};
    use std::path::Path;

    /// D-227: the boundary itself, because it is one byte wide and the OS's own answers never state it — the
    /// kernel refuses 108 bytes and binds 107 (measured 2026-09-27), and everything above the socket (the
    /// daemon's bind, every client's connect, `init`, `doctor`) hangs off this predicate.
    #[test]
    fn the_socket_path_limit_is_one_byte_under_the_kernels() {
        let fits = "/".to_string() + &"a".repeat(SOCKET_PATH_LIMIT - 2);
        assert_eq!(fits.len(), SOCKET_PATH_LIMIT - 1);
        require_socket_path_fits(Path::new(&fits)).expect("107 bytes is the longest path the kernel binds");
        let too_long = "/".to_string() + &"a".repeat(SOCKET_PATH_LIMIT - 1);
        assert_eq!(too_long.len(), SOCKET_PATH_LIMIT);
        let error = require_socket_path_fits(Path::new(&too_long)).expect_err("108 bytes is one over");
        assert!(error.contains("108-byte `sun_path` limit"), "{error}");
        assert!(error.contains("point XDG_STATE_HOME"), "the refusal names the fix: {error}");
    }

    /// D-183: both branches of the state-root row, the refusing one included — a machine that links a new
    /// SQLite cannot produce it, so the rule that says an old one is reported as a failure is asserted here.
    #[test]
    fn the_state_root_row_names_the_linked_sqlite_and_refuses_an_old_one() {
        let root = Path::new("/state/teamagents/v2");
        let (ok, detail) = v2_state_root_row(root, "wal", "FULL", "3.53.2", true);
        assert!(ok, "{detail}");
        assert_eq!(detail, "/state/teamagents/v2 (journal_mode=wal, synchronous=FULL, sqlite=3.53.2)");
        let (ok, detail) = v2_state_root_row(root, "wal", "FULL", "3.49.1", false);
        assert!(!ok, "{detail}");
        assert!(detail.contains("sqlite=3.49.1 lacks the WAL-reset fix 3051003"), "{detail}");
        assert!(detail.contains("DESIGN §4.4"), "the refusal cites the requirement: {detail}");
    }
}
