//! R2-P1 direct-drive reference loop (R06, RV-38): the same kernel, provider
//! edge and basic tools as the persistent runtime, driven without team
//! management. This loop is the eval group-A reference — it exists for
//! evaluation and carries no production recovery promise; it is not a second
//! kernel and reuses `core::kernel` for every model-facing decision.

use crate::gateway::TurnControl;
use crate::observability::trace::Trace;
use crate::providers::{Cancel, ErrorClass, Provider, ProviderEvent};
use crate::tools::{ShellMode, V2Toolkit};
use serde_json::{json, Value as Json};
use std::path::PathBuf;
use std::time::Duration;
use teamagents_core::kernel::*;
use teamagents_core::models::UserConfig;

pub struct ReferenceConfig {
    pub workspace: PathBuf,
    pub artifacts: Option<PathBuf>,
    pub shell_state: Option<PathBuf>,
    /// Trusted session permission mode, never model-controlled (D-41).
    pub permissions: String,
    pub profile: KernelProfile,
    /// Web catalog + bindings; empty bindings expose no web tools.
    pub catalog: UserConfig,
    pub bindings: Vec<String>,
    pub max_steps: usize,
    pub max_retries: usize,
    pub deadline: Option<Duration>,
    pub trace_dir: PathBuf,
    pub run_id: String,
}

#[derive(Debug)]
pub enum ReferenceEnd {
    /// Model issued the single finish call; candidate goes to checks (§8).
    Completed(CompletionCandidate),
    /// Plain reply termination (chat-style; does not settle tasks by itself).
    Reply(String),
    /// Step budget, deadline, or permanent failure with the real reason.
    Failed(String),
}

#[derive(Debug)]
pub struct ReferenceOutcome {
    pub end: ReferenceEnd,
    pub steps: usize,
    pub usage: Usage,
    pub trace_path: PathBuf,
}

/// Run one task through the reference loop. Every model request, attempt,
/// tool receipt and the completion land in `<trace_dir>/<run_id>.trace.jsonl`.
pub async fn run_reference<P: Provider>(
    provider: &P,
    config: ReferenceConfig,
    task: &str,
    mut preview: impl FnMut(ProviderEvent) + Send,
) -> Result<ReferenceOutcome, String> {
    let mode = ShellMode::from_permissions(Some(&config.permissions))?;
    let toolkit = std::sync::Arc::new(V2Toolkit::new(
        config.workspace.clone(),
        config.catalog.clone(),
        config.bindings.clone(),
        config.artifacts.clone(),
        config.shell_state.clone(),
    ));
    let kernel = KernelInstance::new("reference", 0, config.profile.clone());
    let mut trace = Trace::create(&config.trace_dir, &config.run_id)?;
    trace.record(
        "run_start",
        json!({
            "run_id": config.run_id,
            "model": config.profile.model,
            "context_window": config.profile.context_window,
            "workspace": config.workspace.to_string_lossy(),
            "permissions": config.permissions,
            "task": task,
        }),
    )?;
    let cancel = Cancel::new();
    let started = std::time::Instant::now();
    let deadline = config.deadline.map(|d| started + d);
    let mut entries: Vec<ContextEntry> = vec![kernel.user_entry(task, "e1")];
    let mut next_entry = 2u64;
    let mut usage = Usage::default();
    let mut steps = 0usize;
    let control = std::sync::Arc::new(TurnControl::default());

    let end = loop {
        if steps >= config.max_steps {
            break ReferenceEnd::Failed(format!("step budget exhausted (max_steps={})", config.max_steps));
        }
        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            break ReferenceEnd::Failed("deadline exceeded".into());
        }
        steps += 1;
        let request_id = format!("req-{steps}");
        let request = kernel.prepare_request(&entries, &request_id);
        trace.record(
            "request_prepared",
            json!({
                "request_id": request_id,
                "step": steps,
                "est_prompt_tokens": request.est_prompt_tokens,
                "messages": request.messages.len(),
                "wire_bytes": request.to_wire_bytes(),
            }),
        )?;
        // Transport retry belongs to this loop only (one retry owner, §7).
        let outcome = {
            let mut attempt = 0usize;
            loop {
                attempt += 1;
                let attempt_id = format!("{request_id}/a{attempt}");
                trace.record("attempt_start", json!({"attempt_id": attempt_id, "request_id": request_id}))?;
                let remaining = deadline.map(|d| d.saturating_duration_since(std::time::Instant::now()));
                let call = provider.complete(&request, &cancel, &mut preview);
                let result = match remaining {
                    Some(remaining) if !remaining.is_zero() => match tokio::time::timeout(remaining, call).await {
                        Ok(result) => result,
                        Err(_) => {
                            trace.record("attempt_end", json!({"attempt_id": attempt_id, "status": "deadline"}))?;
                            break Err("deadline exceeded".to_string());
                        }
                    },
                    _ => call.await,
                };
                match result {
                    Ok(outcome) => {
                        trace.record(
                            "attempt_end",
                            json!({
                                "attempt_id": attempt_id,
                                "status": "complete",
                                "elapsed_ms": outcome.elapsed_ms,
                                "usage": outcome.response.usage,
                                "raw": outcome.raw,
                            }),
                        )?;
                        break Ok(outcome);
                    }
                    Err(error) => {
                        trace.record(
                            "attempt_end",
                            json!({
                                "attempt_id": attempt_id,
                                "status": format!("{:?}", error.class),
                                "message": error.message,
                                "http_status": error.status,
                            }),
                        )?;
                        let retryable = error.class == ErrorClass::Transient && attempt <= config.max_retries;
                        if !retryable {
                            break Err(match error.class {
                                ErrorClass::ContextOverflow => format!("context overflow: {}", error.message),
                                other => format!("model attempt failed ({other:?}): {}", error.message),
                            });
                        }
                        let wait = error
                            .retry_after
                            .unwrap_or_else(|| Duration::from_millis((500u64 << attempt.min(5)).min(8000)))
                            .min(Duration::from_secs(30));
                        tokio::select! {
                            _ = tokio::time::sleep(wait) => {}
                            _ = cancel.cancelled() => break Err("turn interrupted".to_string()),
                        }
                    }
                }
            }
        };
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(reason) => break ReferenceEnd::Failed(reason),
        };
        if let Some(u) = outcome.response.usage {
            usage.prompt += u.prompt;
            usage.completion += u.completion;
            usage.total += u.total;
        }
        let entry_id = format!("e{next_entry}");
        next_entry += 1;
        let interpretation = kernel.interpret_response(&outcome.response, &entry_id);
        entries.push(interpretation.entry);
        for note in &interpretation.notes {
            let id = format!("e{next_entry}");
            next_entry += 1;
            entries.push(kernel.apply_observation(&Observation::Note(format!("[protocol] {note}")), &id));
        }
        let decision_id = request_id.clone();
        match interpretation.output {
            KernelOutput::Reply(text) => {
                trace.record("reply", json!({"request_id": request_id, "chars": text.chars().count()}))?;
                break ReferenceEnd::Reply(text);
            }
            KernelOutput::Completion(candidate) => {
                trace.record("completion", json!({"request_id": request_id, "candidate": candidate}))?;
                break ReferenceEnd::Completed(candidate);
            }
            KernelOutput::Wait(wait) => {
                break ReferenceEnd::Failed(format!("reference loop cannot wait: {wait}"));
            }
            KernelOutput::ToolIntents(intents) => {
                // Same-response tool calls run sequentially (§6.2).
                for intent in intents {
                    let operation_id = format!("{decision_id}:{}", intent.index);
                    trace.record(
                        "tool_dispatch",
                        json!({"operation_id": operation_id, "tool": intent.name, "args_hash": intent.args_hash}),
                    )?;
                    let receipt = if intent.name == READBACK_TOOL {
                        readback_receipt(&operation_id, &intent, &entries)
                    } else {
                        let toolkit = toolkit.clone();
                        let intent = intent.clone();
                        let operation_id = operation_id.clone();
                        let control = control.clone();
                        tokio::task::spawn_blocking(move || toolkit.call(&operation_id, &intent, &control, mode))
                            .await
                            .map_err(|e| format!("tool worker join: {e}"))?
                    };
                    trace.record("tool_receipt", json!({"receipt": receipt}))?;
                    let id = format!("e{next_entry}");
                    next_entry += 1;
                    entries.push(kernel.apply_observation(
                        &Observation::ToolResult {
                            call_id: intent.call_id.clone(),
                            name: intent.name.clone(),
                            content: receipt.content.clone(),
                            receipt_ref: receipt.operation_id.clone(),
                        },
                        &id,
                    ));
                }
            }
        }
    };
    trace.record(
        "run_end",
        json!({
            "end": format!("{:?}", end).chars().take(2000).collect::<String>(),
            "steps": steps,
            "usage": usage,
            "context_entries": entries.len(),
        }),
    )?;
    Ok(ReferenceOutcome { end, steps, usage, trace_path: trace.path.clone() })
}

/// Built-in readback: page the stored full output of an earlier call.
pub(crate) fn readback_receipt(operation_id: &str, intent: &ToolIntent, entries: &[ContextEntry]) -> ToolReceipt {
    let source = intent.args.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("");
    let found = entries.iter().find(|entry| entry.message["tool_call_id"].as_str() == Some(source));
    let (ok, content) = match found {
        Some(entry) => match page_output(entry.message["content"].as_str().unwrap_or(""), &intent.args) {
            Ok(page) => (true, json!({"output": page}).to_string()),
            Err(error) => (false, json!({"error": error}).to_string()),
        },
        None => (false, json!({"error": format!("no stored output for tool_call_id {source:?}")}).to_string()),
    };
    ToolReceipt {
        operation_id: operation_id.to_string(),
        tool: intent.name.clone(),
        args_hash: intent.args_hash.clone(),
        ok,
        started: true,
        mode: None,
        cwd: None,
        exit_code: None,
        signal: None,
        duration_ms: 0,
        output_ref: None,
        content: content.clone(),
        error: if ok {
            None
        } else {
            Some(ReceiptError { class: "readback".into(), reason: format!("readback failed for {source:?}") })
        },
    }
}

trait WireBytes {
    fn to_wire_bytes(&self) -> usize;
}

impl WireBytes for ModelRequest {
    fn to_wire_bytes(&self) -> usize {
        json!({"messages": self.messages, "tools": self.tools}).to_string().len()
    }
}

/// Workspace file/shell tool schemas for the reference profile (ported from
/// the legacy bound-tool set, wrapped for the wire; readback/finish are
/// kernel built-ins and must not be repeated here).
pub fn basic_tool_schemas(web: bool) -> Vec<Json> {
    let wrap = |name: &str, description: &str, parameters: Json| json!({"type": "function", "function": {"name": name, "description": description, "parameters": parameters}});
    let mut schemas = vec![
        wrap("ls", "List files in your workspace (path defaults to '.').",
            json!({"type":"object","properties":{"path":{"type":"string"}}})),
        wrap("read_file", "Read UTF-8 workspace text, shared /artifacts/ files, or your private /tool-output/ logs in bounded pages. offset is a 1-based line; byte_offset is an absolute byte continuation. Follow next_byte_offset until eof. include_sha256 returns a revision for safe edits.",
            json!({"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer","minimum":1},"limit":{"type":"integer","minimum":1},"byte_offset":{"type":"integer","minimum":0},"include_sha256":{"type":"boolean"}},"required":["path"]})),
        wrap("write_file", "Write a text file in your workspace, creating parent directories. /artifacts/ is shared by the session: write only deliberate deliverables there. /tool-output/ is private and read-only.",
            json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"},"expected_sha256":{"type":"string"}},"required":["path","content"]})),
        wrap("edit_file", "Replace exactly one occurrence of old_string. Ambiguous matches fail unchanged. Pass expected_sha256 from read_file to reject concurrent changes.",
            json!({"type":"object","properties":{"path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},"expected_sha256":{"type":"string"}},"required":["path","old_string","new_string"]})),
        wrap("delete", "Delete a file (a directory when recursive=true) from your workspace.",
            json!({"type":"object","properties":{"path":{"type":"string"},"recursive":{"type":"boolean"}},"required":["path"]})),
        wrap("glob", "Find workspace files matching a glob pattern, e.g. '**/*.py' (max 500 hits).",
            json!({"type":"object","properties":{"pattern":{"type":"string"}},"required":["pattern"]})),
        wrap("grep", "Search workspace files for a pattern; returns matching lines (max 100).",
            json!({"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"}},"required":["pattern"]})),
        wrap("shell", "Run Bash in the current shell_environment mode. approved_scope uses bwrap: network=true needs approval; temporary files/background processes end with the call. full_auto uses the host filesystem/network; services may survive CLI exit. For services redirect stdin/stdout/stderr, record PID, verify in a later call, and stop explicitly. Timeout/cancel kills the active process group. cwd/exports persist separately per mode; a missing cwd skips this call and resets to workspace root. Inspect nonzero exits. Page long output with read_file under private /tool-output/. /tool-output/ and /artifacts/ are virtual file-tool paths.",
            json!({"type":"object","properties":{"command":{"type":"string"},"timeout":{"type":"integer"},"network":{"type":"boolean"}},"required":["command"]})),
    ];
    if web {
        schemas.push(wrap("web_search", "Search the web and return title, source URL, snippet, fetch time (and full content when include_content=true).",
            json!({"type":"object","properties":{"query":{"type":"string"},"max_results":{"type":"integer"},"include_content":{"type":"boolean"}},"required":["query"]})));
        schemas.push(wrap("web_fetch", "Fetch a web page and return title, source URL, fetch time and the readable text body (HTML only; capped).",
            json!({"type":"object","properties":{"url":{"type":"string"},"max_bytes":{"type":"integer"}},"required":["url"]})));
    }
    schemas
}
