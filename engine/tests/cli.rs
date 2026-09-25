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
    assert!(with_hooks.contains("[ok  ] retention"), "the policy is reported: {with_hooks}");

    // a wrong type in [permissions] is an error, not a silent default
    std::fs::write(config.join("config.toml"), "[permissions]\ntrust_project_tools = \"yes\"\n").unwrap();
    let broken = run(&home.join("state"));
    assert!(broken.contains("[FAIL] user config"), "{broken}");
    assert!(broken.contains("must be true/false"), "{broken}");
    let _ = std::fs::remove_dir_all(&home);
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
