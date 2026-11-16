//! Interface contracts use isolated child configuration/state and never contact a user session.
use serde_json::Value;
use std::{
    path::PathBuf,
    process::{Command, Output},
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("ta-interface-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .args(args)
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("XDG_STATE_HOME", self.0.join("state"))
            .current_dir(&self.0)
            .output()
            .unwrap()
    }
    fn json(&self, args: &[&str]) -> Value {
        let output = self.run(args);
        assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice(&output.stdout).expect("a --json success is JSON")
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn unrelated_options_fail_before_configuration_or_state_access() {
    let root = Scratch::new();
    let cases: &[&[&str]] = &[
        &["sessions", "delete", "--id", "default", "--yes", "--dry-run"],
        &["sessions", "prune", "--id", "default", "--dry-run"],
        &["authority", "--grant", "g", "--request", "r"],
        &["runners", "list", "--service", "--yes"],
        &["version", "--effort", "high"],
        &["version", "--after-turn"],
        &["version", "--plain"],
        &["version", "--model", "unused"],
        &["version", "-c", "unused.key=value"],
        &["daemon", "--stop", "-c", "permissions.approvals_reviewer=user"],
        &["mcp", "add", "--name", "fixture", "--command", "python3", "--url", "https://example.com"],
        &["mcp", "remove", "--name", "fixture", "--oauth"],
        &["mcp", "login", "--name", "fixture", "--paste", "--timeout", "1"],
        &["inputs", "paths", "--recipient", "i-other"],
        &["tasks", "--state-root", "missing", "--cwd", "."],
    ];
    for args in cases {
        let output = root.run(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
        assert!(!root.0.join("state").exists(), "{args:?}");
        assert!(!root.0.join("config").exists(), "{args:?}");
    }
}

#[test]
fn dry_run_cannot_delete_a_session_and_mutation_json_is_structured() {
    let root = Scratch::new();
    let report = root.json(&["sessions", "new", "--name", "kept", "--json"]);
    let id = report["entry"]["id"].as_str().or_else(|| report["id"].as_str()).unwrap();
    let output = root.run(&["sessions", "delete", "--id", id, "--yes", "--dry-run", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    let renamed = root.json(&["sessions", "rename", "--id", id, "--name", "renamed", "--json"]);
    assert_eq!(renamed["name"], "renamed");
    assert_eq!(root.json(&["sessions", "archive", "--id", id, "--json"])["archived"], true);
    assert_eq!(root.json(&["sessions", "restore", "--id", id, "--json"])["restored"], true);
    assert_eq!(root.json(&["sessions", "delete", "--id", id, "--stop", "--yes", "--json"])["deleted"], true);
    let automation = root.json(&["automations", "add", "--every", "60", "--prompt", "fixture", "--json"]);
    let id = automation["id"].as_str().unwrap();
    assert_eq!(root.json(&["automations", "pause", "--id", id, "--json"])["enabled"], false);
    assert_eq!(root.json(&["automations", "resume", "--id", id, "--json"])["enabled"], true);
    assert_eq!(root.json(&["automations", "remove", "--id", id, "--yes", "--json"])["removed"], true);
}

#[test]
fn scoped_help_is_local_complete_and_hides_empty_choices() {
    let root = Scratch::new();
    for args in [
        vec!["exec", "--help"],
        vec!["sessions", "delete", "--help"],
        vec!["authority", "grant", "-h"],
        vec!["mcp", "add", "--help"],
    ] {
        let output = root.run(&args);
        assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("Options:"));
        assert!(!text.contains("--execution"));
        assert!(!text.contains("--full-auto"));
    }
    let output = root.run(&["--help"]);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("teamagents update"));
    assert!(text.lines().count() < 30);
    assert!(!root.0.join("state").exists());
}

#[test]
fn mcp_arguments_preserve_dash_leading_values_and_compatibility_execution() {
    let root = Scratch::new();
    let output = root.run(&[
        "mcp",
        "add",
        "--name",
        "fixture",
        "--command",
        "python3",
        "--arg",
        "-u",
        "--arg",
        "--json",
        "--execution",
        "host",
    ]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let text = std::fs::read_to_string(root.0.join("config/teamagents/config.toml")).unwrap();
    let config: toml::Value = text.parse().unwrap();
    assert_eq!(
        config["tools"]["fixture"]["args"].as_array().unwrap(),
        &[toml::Value::String("-u".into()), toml::Value::String("--json".into())]
    );
}

#[test]
fn updater_refuses_conflicts_without_accessing_an_installation() {
    let root = Scratch::new();
    for args in [
        vec!["update", "--status", "--rollback"],
        vec!["update", "--health", "--check"],
        vec!["update", "--status", "--to", "1.0.0"],
        vec!["update", "--request", "missing", "--no-restart"],
        vec!["update", "--status", "--repo", "ignored"],
        vec!["update", "--to", "--json"],
    ] {
        let output = root.run(&args);
        assert!(!output.status.success(), "{args:?}");
        assert!(!root.0.join("state").exists());
    }
}

#[test]
fn resolved_permissions_include_overrides_but_never_project_policy() {
    const CHILD: &str = "TA_INTERFACE_CONFIG_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let (_, project, permissions) = teamagents_engine::config::load_resolved_config(
            &std::env::current_dir().unwrap(),
            &[
                "permissions.approvals_reviewer=user".into(),
                "permissions.reviewer_model=override-model".into(),
                "permissions.trust_project=true".into(),
            ],
        )
        .unwrap();
        assert!(project.trusted);
        assert_eq!(permissions.approvals_reviewer, "user");
        assert_eq!(permissions.reviewer_model.as_deref(), Some("override-model"));
        return;
    }
    let root = Scratch::new();
    let config = root.0.join("config/teamagents");
    let project = root.0.join(".teamagents");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        config.join("config.toml"),
        "[permissions]\napprovals_reviewer='auto_review'\nreviewer_model='file-model'\n",
    )
    .unwrap();
    std::fs::write(
        project.join("config.toml"),
        "[permissions]\napprovals_reviewer='auto_review'\nreviewer_model='project-model'\n",
    )
    .unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "resolved_permissions_include_overrides_but_never_project_policy", "--nocapture"])
        .env(CHILD, "1")
        .env("XDG_CONFIG_HOME", root.0.join("config"))
        .env("XDG_STATE_HOME", root.0.join("state"))
        .current_dir(&root.0)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
