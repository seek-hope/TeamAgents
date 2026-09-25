//! CLI smoke: init / doctor / version and the v2 state root.

use std::process::Command;

/// A daemon the test started, stopped on every exit path including a panic:
/// a leaked daemon would keep running (and hold a coordinator lock) for the
/// rest of the suite.
struct Daemon(std::process::Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn teamagents(args: &[&str], state_home: &std::path::Path, config_home: &std::path::Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
        .args(args)
        .env("XDG_STATE_HOME", state_home)
        .env("XDG_CONFIG_HOME", config_home)
        .output()
        .expect("run cli");
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

/// D-73: the entry point refuses what it does not honour, and refuses it
/// *before* starting anything. A bare word used to fall through to the TUI — a
/// typo'd verb or a pasted prompt silently booted a session and was dropped —
/// and `-v` was accepted with nothing behind it.
#[test]
fn a_bare_word_and_verbose_are_refused_without_starting_a_session() {
    let home = std::env::temp_dir().join(format!("ta-refuse-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let (config_home, state_home, state_root) = (home.join("config"), home.join("state"), home.join("root"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\napi_key_env = \"TA_REFUSE_KEY\"\n\
         base_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .args(args)
            .arg("--state-root")
            .arg(&state_root)
            .env("XDG_STATE_HOME", &state_home)
            .env("XDG_CONFIG_HOME", &config_home)
            .env("TA_REFUSE_KEY", "test-value")
            .output()
            .expect("run cli");
        (output.status.code(), String::from_utf8_lossy(&output.stderr).into_owned())
    };
    for word in ["hello", "frobnicate"] {
        let (code, stderr) = run(&[word]);
        assert_eq!(code, Some(2), "{word}: {stderr}");
        assert!(stderr.contains(word) && stderr.contains("is not an entry point"), "{word}: {stderr}");
        assert!(stderr.contains("teamagents exec"), "the message points at the headless entry: {stderr}");
    }
    let (code, stderr) = run(&["-v"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("-v/--verbose is not supported") && stderr.contains("--version"), "{stderr}");
    // refusing happens before the session: no socket, no daemon, nothing written
    assert!(!state_root.join("daemon.sock").exists(), "a refused argument must not start a session");
    let _ = std::fs::remove_dir_all(&home);
}

/// finding 10/11: doctor runs real probes (bwrap isolation, hook programs)
/// and reports a malformed config instead of silently defaulting it.
#[test]
fn doctor_probes_isolation_and_config_errors() {
    let home = std::env::temp_dir().join(format!("ta-doctor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let config = home.join("config/teamagents");
    std::fs::create_dir_all(&config).unwrap();
    let run = |state: &std::path::Path| -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .arg("doctor")
            .env("XDG_STATE_HOME", state)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .output()
            .expect("run doctor");
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        text
    };
    let clean = run(&home.join("state"));
    assert!(clean.contains("user config"), "{clean}");
    assert!(clean.contains("bubblewrap isolation"), "{clean}");
    if teamagents_engine::tools::bwrap_available() {
        assert!(clean.contains("[ok  ] bubblewrap isolation"), "the isolation probe really runs: {clean}");
    }

    // hooks fail silently at event time, so doctor checks the programs
    std::fs::write(
        config.join("config.toml"),
        "[models.m]\nprovider=\"openai\"\nmodel=\"x\"\n\n[hooks]\nnotify = [\"/nonexistent/notify.sh\"]\npre_tool = [\"/bin/sh\", \"-c\", \"exit 0\"]\n\n[retention]\narchived_days = 30\n",
    )
    .unwrap();
    let with_hooks = run(&home.join("state"));
    assert!(with_hooks.contains("[FAIL] hooks.notify"), "{with_hooks}");
    assert!(with_hooks.contains("[ok  ] hooks.pre_tool"), "{with_hooks}");
    // D-75: retention is accepted (and stays user-config-only) but nothing in this
    // release archives or prunes, so the row says that instead of reporting the
    // numbers as if they were in effect
    assert!(with_hooks.contains("[WARN] retention"), "the policy is reported honestly: {with_hooks}");
    assert!(with_hooks.contains("are not applied"), "{with_hooks}");

    // D-74: a declared MCP service is bound at session start, so doctor names each
    // one and says whether it can run (a mistyped command would stop the boot)
    std::fs::write(
        config.join("config.toml"),
        "[models.m]\nprovider=\"openai\"\nmodel=\"x\"\n\n\
         [tools.good]\nkind = \"mcp\"\ncommand = \"/bin/sh\"\n\n\
         [tools.typo]\nkind = \"mcp\"\ncommand = \"/nonexistent/mcp\"\n\n\
         [tools.remote]\nkind = \"mcp\"\nmcp_transport = \"http\"\nurl = \"https://example.invalid/mcp\"\n",
    )
    .unwrap();
    let with_tools = run(&home.join("state"));
    assert!(with_tools.contains("[ok  ] tools.good"), "{with_tools}");
    assert!(with_tools.contains("[WARN] tools.typo"), "a command that cannot run is reported: {with_tools}");
    assert!(with_tools.contains("not runnable"), "{with_tools}");
    assert!(with_tools.contains("[ok  ] tools.remote"), "{with_tools}");

    // D-78: the web half of the same section is reported too — each declared binding
    // and whether its credential is there (the executor resolves these lazily, so a
    // missing key otherwise shows up only in a tool receipt)
    std::fs::write(
        config.join("config.toml"),
        "[models.m]\nprovider=\"openai\"\nmodel=\"x\"\n\n\
         [tools.search]\nkind = \"web_search\"\nprovider = \"anysearch\"\n\
         url = \"https://api.anysearch.com/v1/search\"\napi_key_env = \"TA_DOCTOR_WEB_KEY\"\n",
    )
    .unwrap();
    let without_key = run(&home.join("state"));
    assert!(without_key.contains("[WARN] tools.search"), "{without_key}");
    assert!(without_key.contains("TA_DOCTOR_WEB_KEY, which is unset"), "{without_key}");
    let with_key = Command::new(env!("CARGO_BIN_EXE_teamagents"))
        .arg("doctor")
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("TA_DOCTOR_WEB_KEY", "test-value")
        .output()
        .expect("run doctor");
    let with_key = String::from_utf8_lossy(&with_key.stdout).into_owned();
    assert!(with_key.contains("[ok  ] tools.search"), "{with_key}");
    assert!(with_key.contains("credential TA_DOCTOR_WEB_KEY is set"), "{with_key}");

    // D-79: a config that declares no web binding at all says so too — the model is then
    // offered neither web_search nor web_fetch, which the README's feature list would
    // otherwise imply is there by default
    std::fs::write(config.join("config.toml"), "[models.m]\nprovider=\"openai\"\nmodel=\"x\"\n").unwrap();
    let without_web = run(&home.join("state"));
    assert!(without_web.contains("[WARN] web tools"), "{without_web}");
    assert!(without_web.contains("offered neither web_search nor web_fetch"), "{without_web}");

    // a wrong type in [permissions] is an error, not a silent default
    std::fs::write(config.join("config.toml"), "[permissions]\ntrust_project_tools = \"yes\"\n").unwrap();
    let broken = run(&home.join("state"));
    assert!(broken.contains("[FAIL] user config"), "{broken}");
    assert!(broken.contains("must be true/false"), "{broken}");
    let _ = std::fs::remove_dir_all(&home);
}

/// Plant a skills directory the doctor can count, and check both the resolving
/// and the missing case: a configured root that is not there is otherwise silent
/// (`skill` answers "no skills configured" only when the model asks).
#[test]
fn doctor_reports_the_skills_registry_and_missing_configured_paths() {
    let root = std::env::temp_dir().join(format!("ta-skills-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (config, state, skills) = (root.join("config"), root.join("state"), root.join("skills"));
    std::fs::create_dir_all(config.join("teamagents")).unwrap();
    // one skill: a directory whose SKILL.md names it
    std::fs::create_dir_all(skills.join("reviewer")).unwrap();
    std::fs::write(skills.join("reviewer/SKILL.md"), "---\nname: reviewer\ndescription: reviews\n---\nbody\n").unwrap();
    std::fs::write(root.join("house-rules.md"), "be careful\n").unwrap();
    let run = |skills_path: &str, instructions: &str| -> String {
        std::fs::write(
            config.join("teamagents/config.toml"),
            format!(
                "skills_paths = [{skills_path}]\ninstruction_files = [{instructions}]\n\n\
                 [models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\napi_key_env = \"TA_SKILLS_KEY\"\n"
            ),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .arg("doctor")
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_STATE_HOME", &state)
            .env("TA_SKILLS_KEY", "test-value")
            .output()
            .expect("run doctor");
        format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
    };
    let good = run(&format!("\"{}\"", skills.display()), &format!("\"{}\"", root.join("house-rules.md").display()));
    assert!(good.contains("[ok  ] skills"), "a resolving root is reported ok: {good}");
    assert!(good.contains("1 skill(s) under 1 configured root(s)"), "{good}");
    assert!(good.contains("[ok  ] instruction files"), "{good}");
    // a root that does not exist is a warning that names it, instead of a skill
    // list that silently stays empty
    let missing = run("\"/nonexistent/skills\"", "\"/nonexistent/rules.md\"");
    assert!(missing.contains("[WARN] skills"), "{missing}");
    assert!(missing.contains("/nonexistent/skills") && missing.contains("never load"), "{missing}");
    assert!(missing.contains("[WARN] instruction files") && missing.contains("/nonexistent/rules.md"), "{missing}");
    // and no configured root at all says where to put one
    let none = run("", "");
    assert!(none.contains("[WARN] skills") && none.contains("skills_paths"), "{none}");
    assert!(!none.contains("instruction files"), "no row without configured files: {none}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Keep diagnostics deterministic on machines without user namespaces.
#[test]
fn doctor_fresh_install_reports_the_missing_requirements() {
    let root = std::env::temp_dir().join(format!("ta-doctor-fresh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let bin = root.join("bin");
    let config = root.join("config/teamagents/config.toml");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    // An empty PATH makes bwrap consistently absent.
    let run = |key: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .arg("doctor")
            .env("PATH", &bin)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_DOCTOR_TEST_KEY", key)
            .output()
            .unwrap();
        (output.status.success(), String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let (ok, text) = run("test-value");
    assert!(!ok && text.contains("[FAIL] user config"), "{text}");
    assert!(text.contains(&config.display().to_string()), "{text}");
    std::fs::write(
        &config,
        "[models.leader_main]\nprovider='openai'\nmodel='test'\napi_key_env='TA_DOCTOR_TEST_KEY'\n",
    )
    .unwrap();
    let (ok, text) = run("test-value");
    assert!(!ok, "missing required bubblewrap must fail doctor: {text}");
    let failures: Vec<_> = text.lines().filter(|line| line.contains("[FAIL]")).collect();
    assert_eq!(failures.len(), 1, "only missing bubblewrap must fail doctor: {text}");
    assert!(failures[0].contains("bubblewrap isolation"), "{text}");
    for key in ["", "   "] {
        let (ok, text) = run(key);
        assert!(!ok && text.contains("[FAIL] model profile leader_main"), "{text}");
        assert!(text.contains("TA_DOCTOR_TEST_KEY"), "{text}");
    }
    std::fs::remove_file(&config).unwrap();
    std::fs::create_dir(&config).unwrap();
    let (ok, text) = run("test-value");
    assert!(!ok && text.contains("cannot read config"), "{text}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_creates_private_config_and_never_overwrites_existing_paths() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = std::env::temp_dir().join(format!("ta-init-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let config = root.join("config with spaces/teamagents/config.toml");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .args(args)
            .env("XDG_CONFIG_HOME", root.join("config with spaces"))
            .env("XDG_STATE_HOME", root.join("state"))
            .output()
            .unwrap()
    };
    assert!(!run(&["init", "--cwd", "/tmp"]).status.success());
    assert!(!config.exists());
    let output = run(&["init"]);
    assert!(output.status.success(), "{output:?}");
    // init prepares only the *v2* state root (R27/A36); it never opens a v1
    // session, so no legacy state directory appears
    assert!(!root.join("state/teamagents/sessions").exists(), "init must not open a legacy session");
    assert!(root.join("state/teamagents/v2/session.sqlite").is_file(), "init must prepare the v2 root");
    let text = std::fs::read_to_string(&config).unwrap();
    let catalog = teamagents_engine::config::parse_user_config(&text).unwrap();
    assert_eq!(catalog.models["leader_main"].context_window, Some(1_000_000));
    assert_eq!(catalog.models["leader_main"].api_key_env.as_deref(), Some("DEEPSEEK_API_KEY"));
    assert_eq!(std::fs::metadata(&config).unwrap().permissions().mode() & 0o777, 0o600);
    std::fs::write(&config, "# existing user content\n").unwrap();
    let output = run(&["init"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("not overwritten"));
    assert_eq!(std::fs::read_to_string(&config).unwrap(), "# existing user content\n");
    std::fs::remove_file(&config).unwrap();
    let target = root.join("missing-target");
    symlink(&target, &config).unwrap();
    assert!(run(&["init"]).status.success());
    assert!(!target.exists(), "a dangling symlink must not be followed");
    assert!(std::fs::symlink_metadata(&config).unwrap().is_symlink());
    std::fs::remove_file(&config).unwrap();
    std::fs::create_dir(&config).unwrap();
    assert!(!run(&["init"]).status.success());
    std::fs::remove_dir_all(root).unwrap();
}

/// A36/R27: `init` prepares an identifiable v2 state root and `doctor` verifies
/// it; a foreign database in the same path is refused rather than reinterpreted
/// (A34), and the legacy layout is only reported.
#[test]
fn init_prepares_the_v2_root_and_doctor_verifies_it() {
    let home = std::env::temp_dir().join(format!("ta-cli-v2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let config = home.join("config");
    std::fs::create_dir_all(config.join("teamagents")).unwrap();
    std::fs::write(
        config.join("teamagents/config.toml"),
        "[models.leader_main]
provider = \"deepseek\"\nprotocol = \"deepseek\"\nmodel = \"deepseek-flash\"\napi_key_env = \"DEEPSEEK_API_KEY\"\ncontext_window = 1000000\n",
    )
    .unwrap();

    let init = teamagents(&["init"], &home, &config);
    assert!(init.contains("state root ready"), "{init}");
    let db = home.join("teamagents/v2/session.sqlite");
    assert!(db.is_file(), "session database missing: {init}");
    let doctor = teamagents(&["doctor"], &home, &config);
    assert!(doctor.contains("v2 state root"), "{doctor}");
    assert!(doctor.contains("journal_mode=wal"), "{doctor}");
    assert!(doctor.contains("synchronous=FULL"), "{doctor}");

    // an explicit --state-root is honoured and reported
    let other = home.join("other-root");
    let custom = teamagents(&["init", "--state-root", other.to_str().unwrap()], &home, &config);
    assert!(custom.contains(other.to_str().unwrap()), "{custom}");

    // a foreign database under the v2 path is refused (A34)
    let foreign = home.join("foreign");
    std::fs::create_dir_all(&foreign).unwrap();
    rusqlite::Connection::open(foreign.join("session.sqlite"))
        .unwrap()
        .execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL); INSERT INTO meta VALUES ('format_id','other-store'),('schema_version','1');")
        .unwrap();
    let refused = teamagents(&["init", "--state-root", foreign.to_str().unwrap()], &home, &config);
    assert!(refused.contains("refusing to reinterpret") || refused.contains("init failed"), "{refused}");

    // the legacy layout is reported (never touched) when it exists
    std::fs::create_dir_all(home.join("teamagents/sessions/old")).unwrap();
    let doctor = teamagents(&["doctor"], &home, &config);
    assert!(doctor.contains("an older release's sessions directory"), "{doctor}");
    std::fs::remove_dir_all(&home).unwrap();
}

/// The documented headless surface end to end through the real binary: plain
/// mode is not a usage error, `-` takes the prompt from stdin, and `--check`
/// really runs the user's acceptance command (it used to be parsed and then
/// dropped) and leaves the verification ledger behind.
#[test]
fn exec_takes_the_prompt_from_stdin_and_runs_the_acceptance_check() {
    use std::io::Write;
    let root = std::env::temp_dir().join(format!("ta-exec-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (config_home, state) = (root.join("config"), root.join("root"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    // a profile that resolves without a network: the turn fails fast against a
    // closed port, which is what makes this test cheap and offline
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_EXEC_TEST_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\ntimeout = 5\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_EXEC_TEST_KEY", "test-value");
    };
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let daemon = daemon
        .args(["daemon", "--state-root"])
        .arg(&state)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start the daemon");
    let _daemon = Daemon(daemon);
    let socket = state.join("daemon.sock");
    for _ in 0..200 {
        if socket.exists() && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(socket.exists(), "the daemon must listen before exec runs");

    let mut exec = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut exec);
    let output = exec
        .args(["exec", "--state-root"])
        .arg(&state)
        .args(["--timeout", "60", "--check", "echo accepted", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(b"instruction from stdin\n")?;
            child.wait_with_output()
        })
        .expect("run exec");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(!stdout.contains("usage:") && !stderr.contains("usage:"), "plain mode is legal: {stdout}{stderr}");
    // the endpoint is closed, so the turn fails and the run is an honest 1
    assert_eq!(output.status.code(), Some(1), "{stdout}{stderr}");
    assert!(stdout.contains("check 1: ok (exit 0)  echo accepted"), "{stdout}");
    assert!(stdout.contains("verification:"), "{stdout}");
    let ledger: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state.join("verification.json")).unwrap()).unwrap();
    assert_eq!(ledger["verification"][0]["command"], "echo accepted");
    assert_eq!(ledger["verification"][0]["ok"], true);
    // the piped prompt really reached the leader's context (§4.2 accept boundary)
    let mut client = teamagents_engine::v2::exec::Client::connect(&socket).expect("client");
    let history = client.history("i-leader").expect("history");
    assert!(
        history.iter().any(|entry| entry["message"]["content"] == "instruction from stdin"),
        "the stdin prompt is what was submitted: {history:?}"
    );
    drop(client);
    let _ = std::fs::remove_dir_all(&root);
}

/// The acceptance checks a user writes in `[[checks]]` really reach the goal the
/// runtime boots: the daemon stores them in the goal's `limits_json`, which is
/// exactly what the completion boundary reads (the gate itself is covered by
/// `v2_driver::configured_checks_gate_the_goal_through_the_config_edge`).
#[test]
fn the_daemon_carries_configured_checks_into_the_goal() {
    let root = std::env::temp_dir().join(format!("ta-checks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (config_home, state) = (root.join("config"), root.join("root"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_CHECKS_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n\n\
         [[checks]]\nid = \"tests\"\ncommand = \"cargo test --offline\"\ntimeout = 600\ninputs = [\"src\"]\n\n\
         [[checks]]\nid = \"docs\"\ncommand = \"test -s README.md\"\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_CHECKS_KEY", "test-value");
    };

    // doctor reports what will gate every goal in this session
    let mut doctor = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut doctor);
    let doctor = doctor.arg("doctor").output().expect("run doctor");
    let text = format!("{}{}", String::from_utf8_lossy(&doctor.stdout), String::from_utf8_lossy(&doctor.stderr));
    assert!(text.contains("2 configured and run at the completion boundary: tests, docs"), "{text}");

    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let daemon = daemon
        .args(["daemon", "--state-root"])
        .arg(&state)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start the daemon");
    let _daemon = Daemon(daemon);
    let socket = state.join("daemon.sock");
    for _ in 0..200 {
        if socket.exists() && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }

    // the goal is created at bootstrap: read the stored limits until it lands
    let db = state.join("session.sqlite");
    let mut limits = String::new();
    for _ in 0..200 {
        if let Ok(control) = teamagents_core::v2::Control::open(&db, "test", false) {
            if let Ok(stored) = control
                .connection()
                .query_row("SELECT limits_json FROM goals LIMIT 1", [], |row| row.get::<_, String>(0))
            {
                limits = stored;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let limits: serde_json::Value = serde_json::from_str(&limits).expect("the goal carries limits");
    let checks = limits["required_checks"].as_array().unwrap_or_else(|| panic!("no checks in {limits}"));
    assert_eq!(checks.len(), 2, "{limits}");
    assert_eq!(checks[0]["id"], "tests");
    assert_eq!(checks[0]["command"], "cargo test --offline");
    assert_eq!(checks[0]["timeout"], 600);
    assert_eq!(checks[0]["inputs"], serde_json::json!(["src"]));
    assert!(checks[0].get("network").is_none(), "an unset flag stays absent: {}", checks[0]);
    assert_eq!(checks[1]["id"], "docs");
    let _ = std::fs::remove_dir_all(&root);
}

/// A14: with no bubblewrap the shell must refuse to run **anything**, and the client's
/// own acceptance commands are no exception — a `--check` command that quietly ran on
/// the host would defeat the isolation the user asked for. The test drives the real
/// binary in a child process whose `PATH` holds no `bwrap` (a child's environment,
/// never the test process's own: other tests resolve tools through `PATH`), with a
/// dead model endpoint, so the turn fails first and the verdict is where the refusal
/// shows up — the same shape a user meets it in.
#[test]
fn an_unisolated_shell_refuses_instead_of_running_on_the_host() {
    if !teamagents_engine::tools::bwrap_available() {
        eprintln!("skipped: this machine has no bwrap, so the sandbox path is unavailable either way");
        return;
    }
    let root = std::env::temp_dir().join(format!("ta-isolation-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (config_home, state, workspace, empty_bin) =
        (root.join("config"), root.join("root"), root.join("ws"), root.join("empty-bin"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&empty_bin).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_ISOLATION_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();

    let run = |workspace: &std::path::Path, state: &std::path::Path, path: &std::path::Path| {
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .args(["exec", "--state-root"])
            .arg(state)
            .args(["--full-auto", "--json", "--timeout", "20", "--cwd"])
            .arg(workspace)
            // the check leaves a file in its cwd when it runs anywhere at all
            .args(["--check", "touch ran-unisolated", "say hi"])
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("xdg-state"))
            .env("TA_ISOLATION_KEY", "test-value")
            .env("PATH", path)
            .output()
            .expect("run exec");
        let printed = String::from_utf8_lossy(&output.stdout).into_owned();
        serde_json::from_str::<serde_json::Value>(printed.trim())
            .unwrap_or_else(|e| panic!("the run must print its JSON report ({e}): {printed:?}"))
    };

    // Control first: the same command with the machine's own PATH runs inside the
    // sandbox and really does leave the file — without this, the absence below would
    // prove nothing (a check whose assertion cannot fail is not a check).
    let control_workspace = root.join("ws-control");
    std::fs::create_dir_all(&control_workspace).unwrap();
    let host_path = std::env::var("PATH").unwrap_or_default();
    let control = run(&control_workspace, &root.join("root-control"), std::path::Path::new(&host_path));
    assert_eq!(control["verification"][0]["ok"], serde_json::json!(true), "the control check runs: {control}");
    assert!(control_workspace.join("ran-unisolated").is_file(), "and leaves its trace: {control}");

    // Then the refusal: no bwrap anywhere, so `which("bwrap")` fails inside the client
    let report = run(&workspace, &state, &empty_bin);
    let verdict = &report["verification"][0];
    assert_eq!(verdict["ok"], serde_json::json!(false), "{report}");
    let error = verdict["error"].as_str().unwrap_or("");
    assert!(error.contains("IsolationUnavailable"), "the refusal names the isolation failure: {report}");
    assert!(!workspace.join("ran-unisolated").exists(), "the check ran somewhere: {report}");
    assert!(std::fs::read_dir(&workspace).unwrap().next().is_none(), "and it must have left nothing behind");
    let _ = std::fs::remove_dir_all(&root);
}

/// `--full-auto` used to be parsed and thrown away by both entry points, so a
/// documented flag did nothing. It now reaches the daemon this client starts,
/// and against a session that is already running (whose mode is fixed, D-41)
/// the client says so with the mode the daemon reports.
#[test]
fn full_auto_reaches_a_started_daemon_and_is_reported_against_a_live_one() {
    fn greeting(socket: &std::path::Path) -> serde_json::Value {
        use std::io::{BufRead, BufReader};
        let stream = std::os::unix::net::UnixStream::connect(socket).expect("connect");
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).expect("greeting");
        serde_json::from_str(&line).expect("greeting JSON")
    }
    let root = std::env::temp_dir().join(format!("ta-full-auto-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (config_home, state) = (root.join("config"), root.join("root"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_FULL_AUTO_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_FULL_AUTO_KEY", "test-value");
    };

    // full-auto on a fresh state root: the daemon this run starts boots in it
    let mut exec = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut exec);
    let output = exec
        .args(["exec", "--state-root"])
        .arg(&state)
        .args(["--full-auto", "--timeout", "5", "hello"])
        .output()
        .expect("run exec");
    let socket = state.join("daemon.sock");
    assert!(socket.exists(), "exec started the daemon");
    assert_eq!(greeting(&socket)["permissions"], "full_auto", "the flag reached the daemon");
    let _ = output;

    // the session keeps its mode: asking again cannot change a running daemon
    let mut again = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut again);
    let output = again
        .args(["exec", "--state-root"])
        .arg(&state)
        .args(["--full-auto", "--timeout", "5", "hello"])
        .output()
        .expect("run exec again");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("already running for this state root in full_auto mode"),
        "the client reports the real mode instead of pretending: {stderr}"
    );
    // Stop that daemon (exec detaches it on purpose) and prove it is gone: a
    // leaked session would keep this state root alive after the test.
    let _ = Command::new("pkill").args(["-f", &format!("daemon --state-root {}", state.display())]).status();
    for _ in 0..100 {
        if std::os::unix::net::UnixStream::connect(&socket).is_err() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        std::os::unix::net::UnixStream::connect(&socket).is_err(),
        "the daemon must stop so the test leaks nothing"
    );

    // D-75: the mode is also a config decision — `[permissions] mode` in the user's
    // own file is the session default, and the flag still wins over it. The key used
    // to be parsed, validated and ignored, so a user who wrote `full_auto` silently
    // ran in approved_scope.
    let write_config = |mode: &str| {
        std::fs::write(
            config_home.join("teamagents/config.toml"),
            format!(
                "[permissions]\nmode = \"{mode}\"\n\n[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
                 api_key_env = \"TA_FULL_AUTO_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n"
            ),
        )
        .unwrap();
    };
    let boot_with = |args: &[&str], state: &std::path::Path| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_teamagents"));
        env(&mut command);
        command.args(["exec", "--state-root"]).arg(state).args(args).args(["--timeout", "5", "hello"]);
        let _ = command.output().expect("run exec");
        let greeting = greeting(&state.join("daemon.sock"));
        let _ = Command::new("pkill").args(["-f", &format!("daemon --state-root {}", state.display())]).status();
        greeting["permissions"].as_str().unwrap_or("").to_string()
    };
    write_config("full_auto");
    assert_eq!(boot_with(&[], &root.join("configured")), "full_auto", "the config sets the session's mode");
    write_config("approved_scope");
    assert_eq!(boot_with(&[], &root.join("default")), "approved_scope", "the documented default stays");
    assert_eq!(
        boot_with(&["--full-auto"], &root.join("flagged")),
        "full_auto",
        "and the flag still asks for host execution for one boot"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `--cwd` used to be dropped when a client started the daemon, so the session
/// (and every instance in it) worked in whatever directory the client happened
/// to be started from — a dogfooding run spent sixty turns exploring the wrong
/// tree before it found the intended one (D-57). The flag now reaches the
/// daemon, and the instance's recorded workspace proves where the tools work.
#[test]
fn cwd_reaches_a_started_daemon_and_is_reported_against_a_live_one() {
    fn greeting(socket: &std::path::Path) -> serde_json::Value {
        use std::io::{BufRead, BufReader};
        let stream = std::os::unix::net::UnixStream::connect(socket).expect("connect");
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).expect("greeting");
        serde_json::from_str(&line).expect("greeting JSON")
    }
    let root = std::env::temp_dir().join(format!("ta-cwd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (config_home, state, first, second) =
        (root.join("config"), root.join("root"), root.join("one"), root.join("two"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    for dir in [&first, &second] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_CWD_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_CWD_KEY", "test-value");
    };

    let mut exec = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut exec);
    exec.args(["exec", "--state-root"])
        .arg(&state)
        .args(["--cwd"])
        .arg(&first)
        .args(["--timeout", "5", "hello"])
        .output()
        .expect("run exec");
    let socket = state.join("daemon.sock");
    let live = greeting(&socket);
    assert_eq!(live["workspace"], first.canonicalize().unwrap().to_string_lossy().as_ref(), "{live}");
    // the instance the daemon drives works in exactly that directory
    let db = state.join("session.sqlite");
    let mut stored = String::new();
    for _ in 0..200 {
        if let Ok(control) = teamagents_core::v2::Control::open(&db, "test", false) {
            if let Ok(value) =
                control
                    .connection()
                    .query_row("SELECT workspace_ref FROM instances LIMIT 1", [], |row| row.get::<_, String>(0))
            {
                stored = value;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert_eq!(stored, first.to_string_lossy(), "the tools' workspace is the requested one");

    // a running session keeps its workspace: the client says so instead of
    // silently working somewhere else
    let mut again = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut again);
    let output = again
        .args(["exec", "--state-root"])
        .arg(&state)
        .args(["--cwd"])
        .arg(&second)
        .args(["--timeout", "5", "hello"])
        .output()
        .expect("run exec again");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("works in") && stderr.contains("--cwd") && stderr.contains(second.to_str().unwrap()),
        "the note names the live workspace and the ignored flag: {stderr}"
    );

    let _ = Command::new("pkill").args(["-f", &format!("daemon --state-root {}", state.display())]).status();
    for _ in 0..100 {
        if std::os::unix::net::UnixStream::connect(&socket).is_err() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(std::os::unix::net::UnixStream::connect(&socket).is_err(), "the daemon must stop");
    let _ = std::fs::remove_dir_all(&root);
}

/// The daemon a user actually starts must authorize its Leader: before D-58 the
/// only grant in a real session was `shell@workspace`, so the model was never
/// offered spawn/delegate/send and the team feature was unreachable from
/// `teamagents`, `exec` and `daemon` alike.
#[test]
fn the_daemon_grants_the_leader_the_team_authority() {
    let root = std::env::temp_dir().join(format!("ta-grants-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (config_home, state) = (root.join("config"), root.join("root"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_GRANTS_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_GRANTS_KEY", "test-value");
    };
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let daemon = daemon
        .args(["daemon", "--state-root"])
        .arg(&state)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start the daemon");
    let _daemon = Daemon(daemon);
    let socket = state.join("daemon.sock");
    for _ in 0..200 {
        if socket.exists() && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }

    // read the grant table through the daemon's own protocol
    use std::io::{BufRead, BufReader, Write};
    let stream = std::os::unix::net::UnixStream::connect(&socket).expect("connect");
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut writer = stream;
    let mut greeting = String::new();
    reader.read_line(&mut greeting).expect("greeting");
    let mut grants = serde_json::Value::Null;
    for _ in 0..200 {
        writer
            .write_all(
                format!(
                    "{}\n",
                    serde_json::json!({"protocol_version": 1, "request_id": "g1", "method": "grants", "params": {}})
                )
                .as_bytes(),
            )
            .expect("write");
        let mut line = String::new();
        reader.read_line(&mut line).expect("reply");
        let reply: serde_json::Value = serde_json::from_str(&line).expect("reply JSON");
        let held = reply["result"]["grants"].as_array().cloned().unwrap_or_default();
        if held.iter().any(|grant| grant["action"] == "manage") {
            grants = reply["result"]["grants"].clone();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let granted = |action: &str| {
        grants
            .as_array()
            .map(|all| {
                all.iter().any(|grant| {
                    grant["subject"] == "i-leader" && grant["action"] == action && grant["resource_scope"] == "session"
                })
            })
            .unwrap_or(false)
    };
    for action in ["manage", "delegate", "message"] {
        assert!(granted(action), "the session must grant the leader {action}@session: {grants}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// One instance whose model cannot be built must not take the session with it.
/// The daemon's provider factory used to panic for such an instance — inside the
/// supervisor's discovery loop, so no further instance was ever driven and every
/// delegating Leader waited forever (D-59). It now returns a provider that fails
/// that instance's own requests permanently: the instance parks through the
/// ordinary classified path (A07) with the reason, and the session lives on.
#[test]
fn an_unbootable_instance_parks_itself_and_the_session_survives() {
    use std::io::{BufRead, BufReader, Write};
    let root = std::env::temp_dir().join(format!("ta-unbootable-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (config_home, state, ws) = (root.join("config"), root.join("root"), root.join("ws"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_UNBOOTABLE_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_UNBOOTABLE_KEY", "test-value");
    };
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let daemon = daemon
        .args(["daemon", "--state-root"])
        .arg(&state)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start the daemon");
    let _daemon = Daemon(daemon);
    let socket = state.join("daemon.sock");
    for _ in 0..200 {
        if socket.exists() && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let stream = std::os::unix::net::UnixStream::connect(&socket).expect("connect");
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut writer = stream;
    let mut greeting = String::new();
    reader.read_line(&mut greeting).expect("greeting");
    let mut command = |id: &str, method: &str, params: serde_json::Value| {
        writer
            .write_all(
                format!(
                    "{}\n",
                    serde_json::json!({"protocol_version": 1, "request_id": id, "command_id": id,
                                       "method": method, "params": params})
                )
                .as_bytes(),
            )
            .expect("write");
        let mut line = String::new();
        reader.read_line(&mut line).expect("reply");
        serde_json::from_str::<serde_json::Value>(&line).expect("reply JSON")
    };

    // an instance whose model is not in the catalog, given work to do
    let broken = command(
        "c-bad",
        "create_instance",
        serde_json::json!({"id": "i-broken", "workspace_ref": ws.to_string_lossy(),
                           "profile": {"model": "gpt-9", "instructions": "never boots"}}),
    );
    assert_eq!(broken["ok"], true, "{broken}");
    command("c-goal", "create_goal", serde_json::json!({"id": "g-broken", "instance_id": "i-broken"}));
    command(
        "c-input",
        "submit_input",
        serde_json::json!({"instance_id": "i-broken", "envelope_id": "env-broken", "text": "do the work"}),
    );

    // it parks with the reason, and the leader's session keeps running
    let mut parked = false;
    let mut reason = String::new();
    for _ in 0..400 {
        let reply = command("c-check", "checkpoint", serde_json::json!({}));
        let instances = reply["result"]["snapshot"]["instances"].as_array().cloned().unwrap_or_default();
        let leader = instances.iter().find(|entry| entry["id"] == serde_json::json!("i-leader"));
        assert_eq!(
            leader.map(|entry| entry["lifecycle"].clone()),
            Some(serde_json::json!("ACTIVE")),
            "the session must live on: {reply}"
        );
        if let Some(broken) = instances.iter().find(|entry| entry["id"] == serde_json::json!("i-broken")) {
            if broken["lifecycle"] == serde_json::json!("PARKED") {
                parked = true;
                // the reason travels with the classified failure the instance
                // parked on, not in the snapshot
                let events = command("c-events", "events", serde_json::json!({"since": 0}));
                reason = events["result"]["events"]
                    .as_array()
                    .map(|events| {
                        events
                            .iter()
                            .filter_map(|event| event["payload"]["reason"].as_str())
                            .collect::<Vec<&str>>()
                            .join(" | ")
                    })
                    .unwrap_or_default();
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(parked, "the unbootable instance parks itself through the classified path");
    assert!(reason.contains("gpt-9"), "the reason names the model: {reason:?}");
    let _ = std::fs::remove_dir_all(&root);
}

/// A socket client for the daemon's JSON-lines protocol, as a test would write it:
/// `authority` is the product's client, and these tests use the same wire shape to
/// set a session up and to read back what the surface did.
struct Rpc {
    reader: std::io::BufReader<std::os::unix::net::UnixStream>,
    writer: std::os::unix::net::UnixStream,
    counter: u64,
}

impl Rpc {
    fn connect(socket: &std::path::Path) -> Rpc {
        use std::io::{BufRead, BufReader};
        let stream = std::os::unix::net::UnixStream::connect(socket).expect("connect");
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        let mut greeting = String::new();
        reader.read_line(&mut greeting).expect("greeting");
        Rpc { reader, writer: stream, counter: 0 }
    }

    fn roundtrip(&mut self, method: &str, params: serde_json::Value, command_id: Option<&str>) -> serde_json::Value {
        use std::io::{BufRead, Write};
        self.counter += 1;
        let mut frame = serde_json::json!({"protocol_version": 1, "request_id": format!("r{}", self.counter),
                                           "method": method, "params": params});
        if let Some(command_id) = command_id {
            frame["command_id"] = serde_json::json!(command_id);
        }
        self.writer.write_all(format!("{frame}\n").as_bytes()).expect("write");
        let mut line = String::new();
        self.reader.read_line(&mut line).expect("reply");
        serde_json::from_str(&line).expect("reply JSON")
    }

    fn call(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.roundtrip(method, params, None)
    }

    fn command(&mut self, id: &str, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.roundtrip(method, params, Some(id))
    }

    fn grants(&mut self) -> Vec<serde_json::Value> {
        self.call("grants", serde_json::json!({}))["result"]["grants"].as_array().cloned().unwrap_or_default()
    }
}

/// The user's authority surface (D-61), end to end: the real binary, the real
/// daemon socket, the real control plane. Before it existed the capability
/// boundary was real but unreachable for a user — `issue_grant`/`revoke_grant` had
/// no caller outside tests, and the daemon's grant view did not even carry the id
/// a revoke must name, so no client could have revoked anything.
#[test]
fn the_authority_surface_grants_and_revokes_through_the_daemon() {
    let root = std::env::temp_dir().join(format!("ta-authority-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (config_home, state, ws) = (root.join("config"), root.join("root"), root.join("ws"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_AUTHORITY_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_AUTHORITY_KEY", "test-value");
    };
    // `teamagents authority …` with the state root this session lives in
    let authority = |args: &[&str]| -> (i32, String, String) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_teamagents"));
        env(&mut command);
        let output = command.args(args).arg("--state-root").arg(&state).output().expect("run authority");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };
    let json = |text: &str| -> serde_json::Value { serde_json::from_str(text).expect("JSON report") };

    // no session yet: the surface says so instead of writing into the void
    let (code, _, stderr) = authority(&["authority"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("start one with"), "the error points at a session: {stderr}");

    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let daemon = daemon
        .args(["daemon", "--state-root"])
        .arg(&state)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start the daemon");
    let _daemon = Daemon(daemon);
    let socket = state.join("daemon.sock");
    for _ in 0..400 {
        if socket.exists() && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let mut rpc = Rpc::connect(&socket);
    // wait for the bootstrap (the leader's own grants)
    for _ in 0..400 {
        if rpc.grants().iter().any(|grant| grant["action"] == serde_json::json!("manage")) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }

    // 1. the list carries what a revoke needs: an id, its issuer and the revision
    let (code, out, stderr) = authority(&["authority", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let listed = json(&out);
    assert!(listed["revision"].as_i64().unwrap_or(0) > 0, "{listed}");
    let leader_shell = listed["grants"]
        .as_array()
        .expect("grants")
        .iter()
        .find(|grant| {
            grant["subject"] == serde_json::json!("i-leader") && grant["action"] == serde_json::json!("shell")
        })
        .expect("the leader's workspace shell");
    assert!(leader_shell["id"].as_str().is_some_and(|id| !id.is_empty()), "the view carries the id: {listed}");
    assert_eq!(leader_shell["issuer"], serde_json::json!("user"), "{listed}");
    assert_eq!(leader_shell["revoked"], serde_json::json!(false), "{listed}");

    // human output is a table a user can read, and it names the id
    let (code, out, _) = authority(&["authority"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("SUBJECT") && out.contains("i-leader") && out.contains(leader_shell["id"].as_str().unwrap()),
        "{out}"
    );

    // 2. a worker the leader spawned holds no shell grant (§5.1)
    let spawned = rpc.command(
        "a-spawn",
        "spawn_instance",
        serde_json::json!({"instance_id": "i-worker-1", "workspace_ref": ws.to_string_lossy(),
                           "profile": {"model": "leader_main", "instructions": "helper"}}),
    );
    assert_eq!(spawned["ok"], true, "{spawned}");
    let holds_shell = |rpc: &mut Rpc| {
        rpc.grants().iter().any(|grant| {
            grant["subject"] == serde_json::json!("i-worker-1")
                && grant["action"] == serde_json::json!("shell")
                && !grant["revoked"].as_bool().unwrap_or(false)
        })
    };
    assert!(!holds_shell(&mut rpc), "a spawned worker holds no shell grant");

    // 3. a parent the session does not know is the control plane's refusal
    let (code, _, stderr) = authority(&[
        "authority",
        "grant",
        "--subject",
        "i-worker-1",
        "--action",
        "shell",
        "--scope",
        "workspace",
        "--parent",
        "g-does-not-exist",
    ]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("parent grant g-does-not-exist"), "{stderr}");

    // 4. the user grants the shared-workspace shell, derived from the leader's own
    //    grant: the user's authority is the root, and the derived grant goes with
    //    the parent (A03)
    let (code, out, stderr) = authority(&[
        "authority",
        "grant",
        "--subject",
        "i-worker-1",
        "--action",
        "shell",
        "--scope",
        "workspace",
        "--parent",
        leader_shell["id"].as_str().unwrap(),
        "--json",
    ]);
    assert_eq!(code, 0, "{stderr}");
    let granted = json(&out);
    let worker_grant = granted["grant_id"].as_str().expect("the new grant id").to_string();
    assert_eq!(granted["subject"], serde_json::json!("i-worker-1"), "{granted}");
    assert_eq!(granted["parent_grant_id"], leader_shell["id"], "the derived grant names its parent");
    assert!(holds_shell(&mut rpc), "the grant is live in the session");
    // the dispatch question A03/A04 re-checks at the linearization point
    let holds_now = |action: &str| {
        teamagents_core::v2::Control::open(&state.join("session.sqlite"), "test", false)
            .expect("control")
            .holds_covering_grant("i-worker-1", action, "workspace")
            .expect("grant read")
    };
    assert!(holds_now("shell"), "the worker may now dispatch the workspace shell");

    // 5. a pair no check consults is refused instead of written
    for (action, scope, expected) in [
        ("shell", "instance:i-worker-1", "no check asks about shell@instance:i-worker-1"),
        ("shel", "workspace", "is not an action the runtime checks"),
    ] {
        let (code, _, stderr) =
            authority(&["authority", "grant", "--subject", "i-worker-1", "--action", action, "--scope", scope]);
        assert_eq!(code, 2, "{stderr}");
        assert!(stderr.contains(expected), "{stderr}");
    }
    // and a subject that does not exist is a warning, not a refusal: instance ids
    // belong to the spawner, so a grant may legitimately come first
    let (code, _, stderr) =
        authority(&["authority", "grant", "--subject", "i-typo", "--action", "message", "--scope", "session"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stderr.contains("no instance \"i-typo\""), "{stderr}");
    assert!(stderr.contains("i-worker-1"), "the warning names the known instances: {stderr}");

    // 6. revoking the parent takes the derived grant with it, and the dispatch
    //    question is false again: revocation is final (A03)
    let (code, out, stderr) =
        authority(&["authority", "revoke", "--grant", &leader_shell["id"].as_str().unwrap()[..12], "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let revocation = json(&out);
    let cascade: Vec<&str> = revocation["revoked"].as_array().unwrap().iter().filter_map(|id| id.as_str()).collect();
    assert!(cascade.contains(&worker_grant.as_str()), "the derived grant goes with its parent: {revocation}");
    assert_eq!(cascade.len(), 2, "{revocation}");
    assert!(!holds_shell(&mut rpc), "no live shell grant for the worker is left");
    assert!(!holds_now("shell"), "a revocation is final: the dispatch question is false again");

    // 7. an id no grant matches is refused before anything is written
    let (code, _, stderr) = authority(&["authority", "revoke", "--grant", "g-nope"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("no grant id starts with"), "{stderr}");
    let _ = std::fs::remove_dir_all(&root);
}

/// `[limits]` in the user config bounds every goal the session creates (D-64): the
/// usage ceiling travels on the goal's `limits` and the deadline is an absolute
/// timestamp the bootstrap derives from the configured minutes. The daemon really
/// applies them: a tiny ceiling parks the leader with the budget as the reason
/// instead of letting the session run.
#[test]
fn configured_limits_reach_the_goal_and_really_bound_the_session() {
    let root = std::env::temp_dir().join(format!("ta-limits-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (config_home, state, ws) = (root.join("config"), root.join("root"), root.join("ws"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    let config = |limits: &str| {
        std::fs::write(
            config_home.join("teamagents/config.toml"),
            format!(
                "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
                 api_key_env = \"TA_LIMITS_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n\n{limits}"
            ),
        )
        .unwrap();
    };
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_LIMITS_KEY", "test-value");
    };
    // doctor says what bounds the goals (nothing, when nothing is configured)
    config("");
    let mut doctor = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut doctor);
    let doctor = doctor.arg("doctor").output().expect("run doctor");
    let text = format!("{}{}", String::from_utf8_lossy(&doctor.stdout), String::from_utf8_lossy(&doctor.stderr));
    assert!(text.contains("[WARN] goal limits"), "an unbounded session is reported: {text}");
    assert!(text.contains("runs until you stop it"), "{text}");

    // a ceiling and a deadline: doctor reports both and the goal carries both
    config("[limits]\nmax_total_tokens = 400000\ndeadline_minutes = 15\n");
    let mut doctor = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut doctor);
    let doctor = doctor.arg("doctor").output().expect("run doctor");
    let text = format!("{}{}", String::from_utf8_lossy(&doctor.stdout), String::from_utf8_lossy(&doctor.stderr));
    assert!(text.contains("max_total_tokens=400000, deadline_minutes=15"), "{text}");
    // a zero is a config error, not a silent default
    config("[limits]\nmax_total_tokens = 0\n");
    let mut doctor = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut doctor);
    let doctor = doctor.arg("doctor").output().expect("run doctor");
    let text = format!("{}{}", String::from_utf8_lossy(&doctor.stdout), String::from_utf8_lossy(&doctor.stderr));
    assert!(text.contains("[FAIL] user config"), "{text}");
    assert!(text.contains("max_total_tokens must be a positive"), "{text}");

    config("[limits]\nmax_total_tokens = 400000\ndeadline_minutes = 15\n");
    let before = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let daemon = daemon
        .args(["daemon", "--state-root"])
        .arg(&state)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start the daemon");
    let _daemon = Daemon(daemon);
    let db = state.join("session.sqlite");
    let mut row = None;
    for _ in 0..400 {
        if let Ok(control) = teamagents_core::v2::Control::open(&db, "test", false) {
            row = control
                .connection()
                .query_row("SELECT limits_json, deadline FROM goals LIMIT 1", [], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<f64>>(1)?))
                })
                .ok();
            if row.is_some() {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let (limits, deadline) = row.expect("the bootstrap creates the goal");
    let limits: serde_json::Value = serde_json::from_str(&limits).unwrap();
    assert_eq!(limits["max_total_tokens"], serde_json::json!(400000), "{limits}");
    assert!(
        limits.get("deadline_minutes").is_none(),
        "the duration is consumed by the bootstrap, never stored on the goal: {limits}"
    );
    let deadline = deadline.expect("a configured deadline lands on the goal");
    let expected = before + 15.0 * 60.0;
    assert!((deadline - expected).abs() < 120.0, "the deadline is ~15 minutes out: {deadline} vs {expected}");
    let _ = std::fs::remove_dir_all(&root);
}

/// The ceiling is not decoration: a goal whose budget cannot cover even one request
/// parks the instance with the budget as the reason (A18) instead of running.
#[test]
fn a_tiny_configured_ceiling_parks_the_session_instead_of_running_it() {
    let root = std::env::temp_dir().join(format!("ta-limit-tiny-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (config_home, state, ws) = (root.join("config"), root.join("root"), root.join("ws"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_TINY_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n\n\
         [limits]\nmax_total_tokens = 4\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_TINY_KEY", "test-value");
    };
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let daemon = daemon
        .args(["daemon", "--state-root"])
        .arg(&state)
        .arg("--cwd")
        .arg(&ws)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start the daemon");
    let _daemon = Daemon(daemon);
    let socket = state.join("daemon.sock");
    for _ in 0..400 {
        if socket.exists() && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let mut rpc = Rpc::connect(&socket);
    // wait for the bootstrap: the socket exists before the leader does
    for _ in 0..400 {
        let checkpoint = rpc.call("checkpoint", serde_json::json!({}));
        if checkpoint["snapshot"]["instances"].as_array().is_some_and(|instances| !instances.is_empty()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    rpc.command(
        "limits-input",
        "submit_input",
        serde_json::json!({"instance_id": "i-leader", "envelope_id": "env-limits", "text": "do something"}),
    );
    // Read the persisted state directly (as the other daemon tests do): the park and
    // its reason are facts in the session database, not a client's view of them.
    let db = state.join("session.sqlite");
    let mut parked = false;
    let mut reason = String::new();
    for _ in 0..600 {
        if let Ok(control) = teamagents_core::v2::Control::open(&db, "test", false) {
            let lifecycle: Option<String> = control
                .connection()
                .query_row("SELECT lifecycle FROM instances WHERE id = 'i-leader'", [], |row| row.get(0))
                .ok();
            reason = control
                .connection()
                .query_row(
                    "SELECT payload_json FROM events WHERE kind = 'instance_lifecycle' ORDER BY sequence DESC LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .ok()
                .and_then(|payload| serde_json::from_str::<serde_json::Value>(&payload).ok())
                .and_then(|payload| payload["reason"].as_str().map(str::to_string))
                .unwrap_or_default();
            if lifecycle.as_deref() == Some("PARKED") && reason.contains("budget") {
                parked = true;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(parked, "a 4-token ceiling parks the leader instead of running it: {reason:?}");
    assert!(reason.contains("budget"), "and the reason names the budget: {reason}");
    let _ = std::fs::remove_dir_all(&root);
}
