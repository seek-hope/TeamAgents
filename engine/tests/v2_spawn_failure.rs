//! R2-P2 spawn-failure honesty (A14): a runner that never reports READY
//! means the command never started. The op must close FAILED with a "spawn"
//! receipt and the driver must keep driving — never die silently with the
//! operation stuck DISPATCH_COMMITTED. Separate test binary: the bogus
//! TEAMAGENTS_RUNNER_BIN must not race the other suites' spawns.

use serde_json::{json, Value as Json};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;
use teamagents_core::kernel::{KernelProfile, ModelRequest, ModelResponse, Usage};
use teamagents_core::models::UserConfig;
use teamagents_engine::providers::{AttemptOutcome, Cancel, Provider, ProviderError, ProviderEvent};
use teamagents_engine::v2::driver::{start, DriverConfig, DriverHandle};

struct ScriptedProvider {
    script: Mutex<VecDeque<Json>>,
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
        let message = self.script.lock().unwrap().pop_front().unwrap_or_else(|| {
            json!({"role": "assistant", "content": "",
                   "tool_calls": [{"id": "finish-1", "type": "function",
                                   "function": {"name": "finish",
                                                "arguments": json!({"status": "success", "summary": "done"}).to_string()}}]})
        });
        Ok(AttemptOutcome {
            response: ModelResponse {
                message,
                usage: Some(Usage { prompt: 3, completion: 2, total: 5 }),
                native: json!({}),
            },
            raw: json!({"scripted": true}),
            elapsed_ms: 1,
        })
    }
}

async fn wait_event(handle: &DriverHandle, kind: &str, timeout_ms: u64) -> Json {
    for _ in 0..(timeout_ms / 25) {
        if let Ok(events) = handle.events(0).await {
            if let Some(event) = events.iter().find(|e| e["kind"] == json!(kind)) {
                return event.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("event {kind} did not arrive within {timeout_ms}ms");
}

#[tokio::test]
async fn runner_spawn_failure_fails_the_op_and_the_driver_survives() {
    // a binary that exits instantly: the runner never persists READY
    std::env::set_var("TEAMAGENTS_RUNNER_BIN", "/bin/false");
    let dir = std::env::temp_dir().join(format!("teamagents-v2-spawn-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let provider = ScriptedProvider {
        script: Mutex::new(VecDeque::from(vec![json!({"role": "assistant", "content": "",
           "tool_calls": [{"id": "c1", "type": "function",
                           "function": {"name": "shell", "arguments": json!({"command": "echo hi"}).to_string()}}]})])),
    };
    let handle = start(DriverConfig {
        session_db: dir.join("session.sqlite"),
        session_id: "s-spawn".into(),
        instance_id: "i-main".into(),
        state_root: dir.join("state"),
        workspace: dir.join("ws"),
        permissions: "full_auto".into(),
        profile: KernelProfile {
            model: "scripted".into(),
            instructions: "test agent".into(),
            tools: vec![json!({"type": "function", "function": {
                "name": "shell", "description": "run a shell command",
                "parameters": {"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}
            }})],
            options: json!({}),
            context_window: Some(128_000),
        },
        provider,
        catalog: UserConfig::default(),
        bindings: vec![],
        max_retries: 1,
        storage_queue: 64,
        poll: Duration::from_millis(15),
        goal_limits: json!({}),
        require_shell_approval: false,
    })
    .await
    .expect("driver start");
    handle.input("run something").await.expect("input");

    // the shell op fails honestly (spawn), then the scripted finish closes
    // the goal — the driver never died on the spawn error
    let completed = wait_event(&handle, "goal_completed", 30_000).await;
    assert_eq!(completed["payload"]["status"], json!("SUCCEEDED"), "{completed}");
    let events = handle.events(0).await.expect("events");
    events
        .iter()
        .find(|e| e["kind"] == json!("operation_completed") && e["payload"]["status"] == json!("FAILED"))
        .expect("a FAILED operation completion");

    // the op is terminally FAILED with an honest spawn receipt (started =
    // false, class = spawn), not stuck DISPATCH_COMMITTED
    let control = teamagents_core::v2::Control::open(&dir.join("session.sqlite"), "s-spawn", false).expect("open");
    let (status, receipt): (String, String) = control
        .connection()
        .query_row("SELECT status, receipt_json FROM operations LIMIT 1", [], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("op row");
    assert_eq!(status, "FAILED");
    let receipt: Json = serde_json::from_str(&receipt).expect("receipt json");
    assert_eq!(receipt["error"]["class"], json!("spawn"), "{receipt}");
    assert_eq!(receipt["started"], json!(false), "{receipt}");
    handle.shutdown().await.expect("shutdown");
    std::fs::remove_dir_all(&dir).ok();
}
