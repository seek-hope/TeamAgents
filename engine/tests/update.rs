//! D-453: the self-update request is a user-approved capability, not a
//! model-executed command. The driver's real control plane, operation and
//! approval path is exercised with a scripted provider and a fake updater.

mod common;

use serde_json::{json, Value as Json};
use std::collections::VecDeque;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use teamagents_core::kernel::{KernelProfile, ModelRequest, ModelResponse, Usage};
use teamagents_core::models::{UpdateConfig, UserConfig};
use teamagents_core::v2::{Command, Identity};
use teamagents_engine::providers::{AttemptOutcome, Cancel, Provider, ProviderError, ProviderEvent};
use teamagents_engine::v2::driver::{start, DriverConfig, DriverHandle};

static ENV: Mutex<()> = Mutex::new(());

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    fn new(vars: &[(&'static str, PathBuf)]) -> EnvGuard {
        let lock = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut saved = vec![];
        for (key, value) in vars {
            saved.push((*key, std::env::var_os(key)));
            std::env::set_var(key, value);
        }
        EnvGuard { _lock: lock, saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

struct ScriptedProvider {
    script: Mutex<VecDeque<Json>>,
}

impl ScriptedProvider {
    fn new(script: Vec<Json>) -> ScriptedProvider {
        ScriptedProvider { script: Mutex::new(VecDeque::from(script)) }
    }
}

impl Provider for ScriptedProvider {
    fn protocol(&self) -> &str {
        "scripted"
    }

    async fn complete(
        &self,
        _request: &ModelRequest,
        _cancel: &Cancel,
        _on_event: &mut (dyn FnMut(ProviderEvent) + Send),
    ) -> Result<AttemptOutcome, ProviderError> {
        let message = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| json!({"role": "assistant", "content": "script exhausted"}));
        Ok(AttemptOutcome {
            response: ModelResponse {
                message,
                usage: Some(Usage { prompt: 3, completion: 2, total: 5 }),
                native: json!({}),
            },
            raw: json!({"scripted": true}),
            elapsed_ms: 1,
            timing: teamagents_engine::providers::StreamTimingReport::default(),
        })
    }
}

fn request_call() -> Json {
    json!({
        "role": "assistant",
        "content": "",
        "tool_calls": [{
            "id": "update-1",
            "type": "function",
            "function": {
                "name": "request_self_update",
                "arguments": json!({
                    "target": "9.9.9",
                    "reason": "the task needs the new release",
                    "follow_up": "continue the task"
                }).to_string()
            }
        }]
    })
}

fn config(root: &Path, provider: ScriptedProvider, policy: &str) -> DriverConfig<ScriptedProvider> {
    let session = root.join("session");
    std::fs::create_dir_all(session.join("instances")).unwrap();
    std::fs::create_dir_all(root.join("ws")).unwrap();
    let catalog = UserConfig {
        update: UpdateConfig { agent_requests: policy.into(), ..Default::default() },
        ..Default::default()
    };
    DriverConfig {
        session_db: session.join("session.sqlite"),
        session_id: "s-update".into(),
        instance_id: "i-main".into(),
        leader_id: "i-main".into(),
        state_root: session.join("instances/i-main"),
        instances_dir: session.join("instances"),
        workspace: root.join("ws"),
        permissions: "full_auto".into(),
        approvals_reviewer: "user".into(),
        reviewer_model: None,
        sandbox: teamagents_engine::tools::SandboxBackend::bubblewrap(),
        profile: KernelProfile {
            images: false,
            tool_output_chars: None,
            context_keep_chars: None,
            model: "scripted".into(),
            instructions: "test agent".into(),
            tools: vec![],
            options: json!({}),
            context_window: Some(128_000),
        },
        provider,
        catalog,
        bindings: vec![],
        max_retries: 1,
        storage_queue: 64,
        poll: Duration::from_millis(15),
        goal_limits: json!({}),
        require_shell_approval: false,
    }
}

fn fake_updater(root: &Path) -> PathBuf {
    let path = root.join("fake-updater.sh");
    let log = root.join("updater.args");
    std::fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\n", log.display())).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

async fn wait_event(handle: &DriverHandle, kind: &str, timeout_ms: u64) -> Json {
    for _ in 0..(timeout_ms / 25) {
        if let Ok(events) = handle.events(0).await {
            if let Some(event) = events.iter().find(|event| event["kind"] == json!(kind)) {
                return event.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("event {kind} did not arrive");
}

async fn wait_for_request(dir: &Path) -> Json {
    for _ in 0..200 {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.path().extension().is_some_and(|extension| extension == "json") {
                    let text = std::fs::read_to_string(entry.path()).unwrap();
                    return serde_json::from_str(&text).unwrap();
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the update request was not persisted");
}

async fn wait_status(handle: &DriverHandle, expected: &str, timeout_ms: u64) -> String {
    for _ in 0..(timeout_ms / 25) {
        let status = handle
            .with_control(|control| {
                control
                    .connection()
                    .query_row("SELECT status FROM operations ORDER BY rowid DESC LIMIT 1", [], |row| {
                        row.get::<_, String>(0)
                    })
                    .ok()
            })
            .await
            .unwrap();
        if status.as_deref() == Some(expected) {
            return expected.to_string();
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the operation did not reach {expected}");
}

#[tokio::test]
async fn a_request_needs_user_approval_and_then_spawns_the_updater() {
    let root = common::TempRoot::new("update-request");
    let updater = fake_updater(&root);
    let _env = EnvGuard::new(&[
        ("TEAMAGENTS_INSTALL_ROOT", root.join("install")),
        ("TEAMAGENTS_BIN_DIR", root.join("bin")),
        ("TEAMAGENTS_UPDATER_BIN", updater),
    ]);
    let handle = start(config(&root, ScriptedProvider::new(vec![request_call()]), "approve")).await.unwrap();
    handle.input("start the update task").await.unwrap();
    let approval = wait_event(&handle, "approval_requested", 5_000).await;
    let approval_id = approval["payload"]["approval_id"].as_str().unwrap().to_string();

    // The model's own identity cannot decide the approval.
    let model_decision = handle
        .with_control({
            let approval_id = approval_id.clone();
            move |control| {
                control.submit(
                    Command {
                        command_id: "model-approve".into(),
                        method: "approve".into(),
                        params: json!({"approval_id": approval_id}),
                    },
                    Identity::Instance("i-main".into()),
                )
            }
        })
        .await
        .unwrap();
    assert!(model_decision.is_err(), "an instance identity must not approve a self-update");

    handle.approve(&approval_id).await.unwrap();
    let request = wait_for_request(&root.join("install/updates/requests")).await;
    assert_eq!(request["target"], "9.9.9");
    assert_eq!(request["reason"], "the task needs the new release");
    assert_eq!(request["follow_up"], "continue the task");
    assert_eq!(request["leader_id"], "i-main");
    let args = std::fs::read_to_string(root.join("updater.args")).unwrap();
    assert!(args.contains("--apply") && args.contains("--request"), "the updater was not spawned: {args}");
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn policy_off_refuses_the_request_without_creating_a_request() {
    let root = common::TempRoot::new("update-policy");
    let updater = fake_updater(&root);
    let _env = EnvGuard::new(&[
        ("TEAMAGENTS_INSTALL_ROOT", root.join("install")),
        ("TEAMAGENTS_BIN_DIR", root.join("bin")),
        ("TEAMAGENTS_UPDATER_BIN", updater),
    ]);
    let handle = start(config(&root, ScriptedProvider::new(vec![request_call()]), "off")).await.unwrap();
    handle.input("start the update task").await.unwrap();
    assert_eq!(wait_status(&handle, "FAILED", 5_000).await, "FAILED");
    let requests = root.join("install/updates/requests");
    assert!(
        !requests.exists() || std::fs::read_dir(&requests).unwrap().next().is_none(),
        "a policy-disabled request must not be persisted"
    );
    assert!(!root.join("updater.args").exists(), "the updater must not run when the policy is off");
    handle.shutdown().await.unwrap();
}

#[test]
fn the_update_cli_reports_status_and_installs_from_an_archive() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let root = common::TempRoot::new("update-cli");
    let install = root.join("install");
    let bin = root.join("bin");
    let state = root.join("state");
    let release = root.join("release");
    let package = "teamagents-9.9.9-x86_64-unknown-linux-musl";
    let dir = release.join(package);
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["teamagents", "teamagents-tui"] {
        let path = dir.join(name);
        std::fs::write(
            &path,
            "#!/bin/sh\ncase \"$1\" in version) echo \"teamagents 9.9.9\" ;; update) echo '{\"ok\":true}' ;; *) exit 0 ;; esac\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let archive = release.join(format!("{package}.tar.gz"));
    assert!(Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&release)
        .arg(package)
        .status()
        .unwrap()
        .success());
    let digest = String::from_utf8(Command::new("sha256sum").arg(&archive).output().unwrap().stdout).unwrap();
    let digest = digest.split_whitespace().next().unwrap();
    std::fs::write(
        release.join("SHA256SUMS"),
        format!("{digest}  {}\n", archive.file_name().unwrap().to_string_lossy()),
    )
    .unwrap();
    let cli = env!("CARGO_BIN_EXE_teamagents");
    let status = Command::new(cli)
        .args(["update", "--status", "--install-root"])
        .arg(&install)
        .arg("--bin-dir")
        .arg(&bin)
        .arg("--json")
        .output()
        .unwrap();
    assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
    let report: Json = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(report["state"]["status"], "idle");
    let applied = Command::new(cli)
        .args(["update", "--to", "9.9.9", "--archive"])
        .arg(&archive)
        .arg("--install-root")
        .arg(&install)
        .arg("--bin-dir")
        .arg(&bin)
        .arg("--state-root")
        .arg(&state)
        .arg("--no-restart")
        .arg("--json")
        .output()
        .unwrap();
    assert!(applied.status.success(), "{}", String::from_utf8_lossy(&applied.stderr));
    let report: Json = serde_json::from_slice(&applied.stdout).unwrap();
    assert_eq!(report["version"], "9.9.9");
    let status = Command::new(cli)
        .args(["update", "--status", "--install-root"])
        .arg(&install)
        .arg("--bin-dir")
        .arg(&bin)
        .arg("--json")
        .output()
        .unwrap();
    let report: Json = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(report["current"]["version"], "9.9.9");
}
