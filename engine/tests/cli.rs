//! CLI smoke: version / validate / sessions.

use std::process::Command;

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

#[test]
fn version_validate_and_sessions_smoke() {
    let home = std::env::temp_dir().join(format!("ta-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // 配置目录也要隔离：否则 `validate` 的结果取决于这台机器上有没有
    // ~/.config/teamagents/config.toml（CI 上没有，profile 就成了未知）
    let config = home.join("config");
    std::fs::create_dir_all(config.join("teamagents")).unwrap();
    std::fs::write(
        config.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n",
    )
    .unwrap();

    let version = teamagents(&["version"], &home, &config);
    assert!(version.contains("teamagents-core"), "{version}");
    for flag in ["--help", "-h", "--version", "-V"] {
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .arg(flag)
            .env("XDG_STATE_HOME", home.join("unused-state"))
            .env("XDG_CONFIG_HOME", home.join("unused-config"))
            .output().unwrap();
        assert!(output.status.success(), "{flag}: {output:?}");
        assert!(!output.stdout.is_empty());
        assert!(!home.join("unused-state").exists());
        assert!(!home.join("unused-config").exists());
    }

    let spec_path = home.join("spec.json");
    std::fs::write(
        &spec_path,
        r#"{"leader_id":"leader","agents":[{"id":"leader","name":"L","role":"leader",
            "runtime_kind":"deepagents","model_profile":"leader_main"}]}"#,
    )
    .unwrap();
    let validated = teamagents(&["validate", spec_path.to_string_lossy().as_ref()], &home, &config);
    assert!(validated.contains("ok:"), "{validated}");

    let bad_path = home.join("bad.json");
    std::fs::write(&bad_path, r#"{"leader_id":"ghost","agents":[]}"#).unwrap();
    let rejected = teamagents(&["validate", bad_path.to_string_lossy().as_ref()], &home, &config);
    assert!(rejected.contains("invalid:"), "{rejected}");

    let sessions = teamagents(&["sessions"], &home, &config);
    assert!(sessions.contains("会话"), "{sessions}");

    // YAML TeamSpec (the format examples/team.yaml uses) validates too
    let yaml_path = home.join("team.yaml");
    std::fs::write(
        &yaml_path,
        "leader_id: leader\nagents:\n  - id: leader\n    name: L\n    role: leader\n    runtime_kind: deepagents\n    model_profile: leader_main\n",
    )
    .unwrap();
    let yaml_ok = teamagents(&["validate", yaml_path.to_string_lossy().as_ref()], &home, &config);
    assert!(yaml_ok.contains("ok:"), "{yaml_ok}");
    let _ = std::fs::remove_dir_all(&home);
}

/// finding 10/11: doctor runs real probes (bwrap, codex app-server + schema)
/// and reports a malformed config instead of silently defaulting it.
#[test]
fn doctor_probes_isolation_codex_and_config_errors() {
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
    assert!(clean.contains("codex app-server"), "{clean}");
    if teamagents_engine::tools::bwrap_available() {
        assert!(clean.contains("[ok  ] bubblewrap isolation"), "the isolation probe really runs: {clean}");
    }
    if teamagents_engine::tools::which("codex").is_some() {
        // 没装 codex 的机器上 doctor 不打印 schema 行（如实报 "codex CLI not found"）
        assert!(clean.contains("codex protocol schema"), "{clean}");
        assert!(clean.contains("[ok  ] codex protocol schema"), "the schema is generated from the CLI: {clean}");
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

/// Keep diagnostics deterministic on machines without Codex or user namespaces.
#[test]
fn doctor_fresh_install_and_optional_codex() {
    let root = std::env::temp_dir().join(format!("ta-doctor-fresh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let bin = root.join("bin");
    let config = root.join("config/teamagents/config.toml");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    // An empty PATH makes required bwrap and optional Codex consistently absent.
    let run = |key: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .arg("doctor")
            .env("PATH", &bin)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_DOCTOR_TEST_KEY", key)
            .output().unwrap();
        (output.status.success(), String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let (ok, text) = run("test-value");
    assert!(!ok && text.contains("[FAIL] user config"), "{text}");
    assert!(text.contains(&config.display().to_string()), "{text}");
    std::fs::write(&config,
        "[models.leader_main]\nprovider='openai'\nmodel='test'\napi_key_env='TA_DOCTOR_TEST_KEY'\n").unwrap();
    let (ok, text) = run("test-value");
    assert!(!ok, "missing required bubblewrap must fail doctor: {text}");
    let failures: Vec<_> = text.lines().filter(|line| line.contains("[FAIL]")).collect();
    assert_eq!(failures.len(), 1, "only missing bubblewrap must fail doctor: {text}");
    assert!(failures[0].contains("bubblewrap isolation"), "{text}");
    assert!(text.contains("[WARN] codex app-server"), "{text}");
    for key in ["", "   "] {
        let (ok, text) = run(key);
        assert!(!ok && text.contains("[FAIL] model profile leader_main"), "{text}");
        assert!(text.contains("TA_DOCTOR_TEST_KEY"), "{text}");
    }
    std::fs::remove_file(&config).unwrap();
    std::fs::create_dir(&config).unwrap();
    let (ok, text) = run("test-value");
    assert!(!ok && text.contains("无法读取配置"), "{text}");
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
            .output().unwrap()
    };
    assert!(!run(&["init", "--cwd", "/tmp"]).status.success());
    assert!(!config.exists());
    let output = run(&["init"]);
    assert!(output.status.success(), "{output:?}");
    assert!(!root.join("state").exists(), "init must not open a session");
    let text = std::fs::read_to_string(&config).unwrap();
    let catalog = teamagents_engine::config::parse_user_config(&text).unwrap();
    assert_eq!(catalog.models["leader_main"].context_window, Some(1_000_000));
    assert_eq!(catalog.models["leader_main"].api_key_env.as_deref(), Some("DEEPSEEK_API_KEY"));
    assert_eq!(std::fs::metadata(&config).unwrap().permissions().mode() & 0o777, 0o600);
    std::fs::write(&config, "# existing user content\n").unwrap();
    let output = run(&["init"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("未覆盖"));
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
