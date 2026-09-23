//! R2-P1 reference-loop end-to-end with a scripted provider (no real model):
//! tool intents execute, receipts import, readback pages stored outputs,
//! plain replies terminate, transient attempts retry once-owned, and the
//! trace carries every request/receipt/completion.

use serde_json::{json, Value as Json};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use teamagents_core::kernel::{KernelProfile, ModelRequest, ModelResponse, Usage};
use teamagents_core::models::UserConfig;
use teamagents_engine::providers::{AttemptOutcome, Cancel, ErrorClass, Provider, ProviderError, ProviderEvent};
use teamagents_engine::reference::{basic_tool_schemas, run_reference, ReferenceConfig, ReferenceEnd};

struct ScriptedProvider {
    script: Mutex<VecDeque<Result<Json, (ErrorClass, &'static str)>>>,
}

impl ScriptedProvider {
    fn new(steps: Vec<Result<Json, (ErrorClass, &'static str)>>) -> ScriptedProvider {
        ScriptedProvider { script: Mutex::new(steps.into_iter().collect()) }
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
        let next = self.script.lock().unwrap().pop_front().expect("scripted provider exhausted");
        match next {
            Ok(message) => Ok(AttemptOutcome {
                response: ModelResponse {
                    message,
                    usage: Some(Usage { prompt: 3, completion: 2, total: 5 }),
                    native: json!({}),
                },
                raw: json!({"scripted": true}),
                elapsed_ms: 1,
            }),
            Err((class, message)) => {
                Err(ProviderError { class, message: message.into(), retry_after: None, status: None })
            }
        }
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("teamagents-rebuild-p1-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn config(workspace: PathBuf, trace_dir: PathBuf) -> ReferenceConfig {
    ReferenceConfig {
        workspace,
        artifacts: None,
        shell_state: None,
        permissions: "full_auto".into(),
        profile: KernelProfile {
            model: "scripted".into(),
            instructions: "test agent".into(),
            tools: basic_tool_schemas(false, false),
            options: json!({}),
            context_window: Some(1_000_000),
        },
        catalog: UserConfig::default(),
        bindings: vec![],
        max_steps: 12,
        max_retries: 2,
        deadline: Some(Duration::from_secs(60)),
        trace_dir,
        run_id: "test".into(),
    }
}

fn tool_call(id: &str, name: &str, arguments: &str) -> Json {
    json!({"role":"assistant","tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":arguments}}]})
}

fn finish(status: &str, summary: &str) -> Json {
    tool_call(
        "fin1",
        "finish",
        &json!({"status": status, "summary": summary, "evidence": ["ran echo"], "unverified": []}).to_string(),
    )
}

fn run(
    provider: &ScriptedProvider,
    config: ReferenceConfig,
    task: &str,
) -> teamagents_engine::reference::ReferenceOutcome {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    rt.block_on(run_reference(provider, config, task, |_| {})).unwrap()
}

#[test]
fn tool_intents_execute_and_finish_completes() {
    let workspace = temp_dir("exec");
    let trace = temp_dir("trace");
    let provider = ScriptedProvider::new(vec![
        Ok(tool_call("call1", "shell", r#"{"command":"echo hello > note.txt && cat note.txt","timeout":10}"#)),
        Ok(finish("success", "wrote note.txt")),
    ]);
    let outcome = run(&provider, config(workspace.clone(), trace), "write a note");
    match outcome.end {
        ReferenceEnd::Completed(candidate) => {
            assert_eq!(candidate.outcome, teamagents_core::kernel::Outcome::Success);
            assert_eq!(candidate.summary, "wrote note.txt");
        }
        other => panic!("expected completion, got {other:?}"),
    }
    assert_eq!(std::fs::read_to_string(workspace.join("note.txt")).unwrap().trim(), "hello");
    assert_eq!(outcome.usage.total, 10);
    let trace_text = std::fs::read_to_string(&outcome.trace_path).unwrap();
    for kind in
        ["run_start", "request_prepared", "attempt_end", "tool_dispatch", "tool_receipt", "completion", "run_end"]
    {
        assert!(trace_text.contains(kind), "trace missing {kind}");
    }
    // the shell receipt is structured: started, mode, cwd, exit code
    let receipt_line = trace_text.lines().find(|l| l.contains("tool_receipt")).unwrap();
    let receipt: Json = serde_json::from_str(receipt_line).unwrap();
    let receipt = &receipt["receipt"];
    assert_eq!(receipt["ok"], true);
    assert_eq!(receipt["started"], true);
    assert_eq!(receipt["mode"], "full_auto");
    assert_eq!(receipt["exit_code"], 0);
    assert!(receipt["cwd"].as_str().unwrap().contains("teamagents-rebuild-p1-exec"));
    assert_eq!(receipt["tool"], "shell");
    assert!(!receipt["args_hash"].as_str().unwrap().is_empty());
}

#[test]
fn readback_pages_masked_outputs() {
    let workspace = temp_dir("readback");
    let trace = temp_dir("trace");
    let big = "0123456789".repeat(3_000); // 30k chars > per-call cap
    let provider = ScriptedProvider::new(vec![
        Ok(tool_call("call1", "shell", &json!({"command": format!("printf %s {big}"), "timeout": 10}).to_string())),
        Ok(tool_call("call2", "read_history", r#"{"tool_call_id":"call1","offset":29990,"limit":20}"#)),
        Ok(finish("success", "paged the output")),
    ]);
    let outcome = run(&provider, config(workspace, trace), "big output");
    assert!(matches!(outcome.end, ReferenceEnd::Completed(_)));
    let trace_text = std::fs::read_to_string(&outcome.trace_path).unwrap();
    let readback = trace_text
        .lines()
        .filter(|l| l.contains("tool_receipt"))
        .map(|l| serde_json::from_str::<Json>(l).unwrap())
        .find(|r| r["receipt"]["tool"] == "read_history")
        .expect("readback receipt");
    assert_eq!(readback["receipt"]["ok"], true);
    // the receipt carries the model-facing envelope {"output": <page>}
    let envelope: Json = serde_json::from_str(readback["receipt"]["content"].as_str().unwrap()).unwrap();
    let page = &envelope["output"];
    // the stored shell result is the {"output": ...} envelope: 30_000 + 13
    assert_eq!(page["total_chars"], json!(30_013));
    assert_eq!(page["offset"], json!(29_990));
    assert_eq!(page["output"].as_str().unwrap().len(), 20);
}

#[test]
fn plain_reply_terminates_without_completion() {
    let workspace = temp_dir("reply");
    let trace = temp_dir("trace");
    let provider = ScriptedProvider::new(vec![Ok(json!({"role":"assistant","content":"the answer is 4"}))]);
    let outcome = run(&provider, config(workspace, trace), "2+2?");
    match outcome.end {
        ReferenceEnd::Reply(text) => assert_eq!(text, "the answer is 4"),
        other => panic!("expected reply, got {other:?}"),
    }
}

#[test]
fn transient_attempt_retries_once_owned_then_permanent_fails() {
    let workspace = temp_dir("retry");
    let trace = temp_dir("trace");
    let provider = ScriptedProvider::new(vec![
        Err((ErrorClass::Transient, "connection reset")),
        Ok(finish("success", "after retry")),
    ]);
    let outcome = run(&provider, config(workspace, trace.clone()), "retry me");
    assert!(matches!(outcome.end, ReferenceEnd::Completed(_)));
    let trace_text = std::fs::read_to_string(outcome.trace_path).unwrap();
    assert_eq!(trace_text.matches("attempt_start").count(), 2);

    let workspace = temp_dir("perm");
    let provider = ScriptedProvider::new(vec![Err((ErrorClass::Permanent, "bad request"))]);
    let outcome = run(&provider, config(workspace, temp_dir("trace2")), "fail me");
    match outcome.end {
        ReferenceEnd::Failed(reason) => assert!(reason.contains("bad request")),
        other => panic!("expected failure, got {other:?}"),
    }
}

#[test]
fn step_budget_is_bounded() {
    let workspace = temp_dir("steps");
    let trace = temp_dir("trace");
    let provider = ScriptedProvider::new(
        (0..20).map(|i| Ok(tool_call(&format!("c{i}"), "shell", r#"{"command":"true","timeout":5}"#))).collect(),
    );
    let outcome = run(&provider, config(workspace, trace), "loop forever");
    match outcome.end {
        ReferenceEnd::Failed(reason) => assert!(reason.contains("step budget"), "reason: {reason}"),
        other => panic!("expected failure, got {other:?}"),
    }
    assert!(outcome.steps <= 12);
}
