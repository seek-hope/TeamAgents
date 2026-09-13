//! CLI smoke: version / validate / sessions (ts/test/cancel-cli.test.ts port).

use std::process::Command;

fn teamagents(args: &[&str], state_home: &std::path::Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
        .args(args)
        .env("XDG_STATE_HOME", state_home)
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

    let version = teamagents(&["version"], &home);
    assert!(version.contains("teamagents-core"), "{version}");

    let spec_path = home.join("spec.json");
    std::fs::write(
        &spec_path,
        r#"{"leader_id":"leader","agents":[{"id":"leader","name":"L","role":"leader",
            "runtime_kind":"deepagents","model_profile":"leader_main"}]}"#,
    )
    .unwrap();
    let validated = teamagents(&["validate", spec_path.to_string_lossy().as_ref()], &home);
    assert!(validated.contains("ok:"), "{validated}");

    let bad_path = home.join("bad.json");
    std::fs::write(&bad_path, r#"{"leader_id":"ghost","agents":[]}"#).unwrap();
    let rejected = teamagents(&["validate", bad_path.to_string_lossy().as_ref()], &home);
    assert!(rejected.contains("invalid:"), "{rejected}");

    let sessions = teamagents(&["sessions"], &home);
    assert!(sessions.contains("会话"), "{sessions}");

    // YAML TeamSpec (the format examples/team.yaml uses) validates too
    let yaml_path = home.join("team.yaml");
    std::fs::write(
        &yaml_path,
        "leader_id: leader\nagents:\n  - id: leader\n    name: L\n    role: leader\n    runtime_kind: deepagents\n    model_profile: leader_main\n",
    )
    .unwrap();
    let yaml_ok = teamagents(&["validate", yaml_path.to_string_lossy().as_ref()], &home);
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
    assert!(clean.contains("codex protocol schema"), "{clean}");
    if teamagents_engine::tools::bwrap_available() {
        assert!(clean.contains("[ok  ] bubblewrap isolation"), "the isolation probe really runs: {clean}");
    }
    if teamagents_engine::tools::which("codex").is_some() {
        assert!(clean.contains("[ok  ] codex protocol schema"), "the schema is generated from the CLI: {clean}");
    }

    // a wrong type in [permissions] is an error, not a silent default
    std::fs::write(config.join("config.toml"), "[permissions]\ntrust_project_tools = \"yes\"\n").unwrap();
    let broken = run(&home.join("state"));
    assert!(broken.contains("[FAIL] user config"), "{broken}");
    assert!(broken.contains("must be true/false"), "{broken}");
    let _ = std::fs::remove_dir_all(&home);
}
