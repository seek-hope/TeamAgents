//! The auth commands are user-scoped; a signed-out subscription must never use an API key.
use serde_json::Value;
use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use uuid::Uuid;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("teamagents-auth-cli-{}", Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        fs::create_dir(dir.join("work")).unwrap();
        Self(dir)
    }
    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_teamagents"));
        c.env_clear().env("HOME", &self.0).env("PATH", "/usr/bin:/bin").current_dir(self.0.join("work"));
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn local_auth_commands_work_without_a_workspace_or_daemon() {
    let fixture = Fixture::new();
    for args in [["auth", "status"], ["auth", "logout"]] {
        let result = fixture.command().current_dir(&fixture.0).args(args).output().unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    }
    assert!(!fixture.0.join(".local/state/teamagents-swarm/sessions").exists());
    assert!(!fixture.run(&["auth", "other"]).status.success());
}

#[test]
fn subscription_client_refuses_an_already_running_api_daemon_before_submit() {
    let fixture = Fixture::new();
    let started = fixture.command().env("TEAMAGENTS_PROVIDER", "api").arg("members").output().unwrap();
    assert!(started.status.success());
    let rejected =
        fixture.command().env("TEAMAGENTS_PROVIDER", "chatgpt").args(["exec", "--detach", "hello"]).output().unwrap();
    let list = fixture.run(&["tasks", "--json"]);
    assert!(fixture.run(&["stop"]).status.success());
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("No task was submitted"));
    assert_eq!(serde_json::from_slice::<Value>(&list.stdout).unwrap(), serde_json::json!([]));
}

#[test]
fn signed_out_plan_reports_login_action_without_contacting_api_provider() {
    let fixture = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let result = fixture
        .command()
        .env("TEAMAGENTS_PROVIDER", "chatgpt")
        .env("OPENAI_BASE_URL", format!("http://{}/v1", listener.local_addr().unwrap()))
        .env("OPENAI_API_KEY", "fixture-key")
        .env("TEAMAGENTS_MODEL", "fixture-model")
        .args(["exec", "--json", "--timeout", "10", "hello"])
        .output()
        .unwrap();
    let stopped = fixture.run(&["stop"]);
    assert!(stopped.status.success());
    assert!(!result.status.success());
    let task: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(task["status"], "failed");
    assert!(task["stage_error"].as_str().unwrap().contains("auth login"), "{task}");
    assert!(!task.to_string().contains("fixture-key"));
    assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
}
