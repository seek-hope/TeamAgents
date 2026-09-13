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
    let _ = std::fs::remove_dir_all(&home);
}
