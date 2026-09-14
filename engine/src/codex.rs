//! Codex execution member backend (codex.py): one `codex app-server` JSON-RPC
//! process, one thread per member, approvals parked in the core.

use crate::core_client::CoreClient;
use crate::gateway::{operation_hash, ApprovalGate, ToolGateway};
use crate::runtime::{AgentRunner, Notify};
use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;
use teamagents_core::control::TurnOutcome;
use teamagents_core::models::{ApprovalRequest, ApprovalStatus, TurnRun, TurnStatus};

type Pending = Arc<Mutex<HashMap<u64, Sender<Result<Json, String>>>>>;
type NotifyHandler = Arc<dyn Fn(Json) + Send + Sync>;
type RequestHandler = Arc<dyn Fn(Json) -> Json + Send + Sync>;

#[derive(Default)]
pub struct AppServerOptions {
    pub codex_bin: Option<String>,
    pub codex_home: Option<String>,
    pub env: Vec<(String, String)>,
    pub config_overrides: Vec<(String, Json)>,
}

/// Minimal JSON-RPC client for one app-server process.
pub struct CodexAppServer {
    opts: AppServerOptions,
    cwd: std::path::PathBuf,
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    pending: Pending,
    notify: Mutex<Option<NotifyHandler>>,
    on_request: Mutex<Option<RequestHandler>>,
    next_id: AtomicU64,
    stderr_lines: Mutex<Vec<String>>,
}

impl CodexAppServer {
    pub fn new(cwd: &std::path::Path, opts: AppServerOptions) -> Arc<Self> {
        Arc::new(Self {
            opts,
            cwd: cwd.to_path_buf(),
            child: Mutex::new(None),
            stdin: Mutex::new(None),
            pending: Arc::new(Mutex::new(HashMap::new())),
            notify: Mutex::new(None),
            on_request: Mutex::new(None),
            next_id: AtomicU64::new(1),
            stderr_lines: Mutex::new(vec![]),
        })
    }

    pub fn set_handlers(&self, notify: NotifyHandler, on_request: RequestHandler) {
        *self.notify.lock().unwrap() = Some(notify);
        *self.on_request.lock().unwrap() = Some(on_request);
    }

    pub fn start(self: &Arc<Self>) -> Result<(), String> {
        let mut args = vec!["app-server".to_string()];
        for (key, value) in &self.opts.config_overrides {
            args.push("-c".into());
            args.push(match value {
                Json::String(s) => format!("{key}={s}"),
                other => format!("{key}={other}"),
            });
        }
        let mut command = Command::new(self.opts.codex_bin.clone().unwrap_or_else(|| "codex".into()));
        command
            .args(&args)
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // own process group (Python: start_new_session + killpg): the shell
        // commands an app-server spawns are its children, and killing only the
        // direct child would leave them running
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        if let Some(home) = &self.opts.codex_home {
            command.env("CODEX_HOME", home);
        }
        for (key, value) in &self.opts.env {
            command.env(key, value);
        }
        let mut child = command.spawn().map_err(|e| format!("cannot start codex app-server: {e}"))?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take().ok_or("no stderr")?;
        *self.stdin.lock().unwrap() = child.stdin.take();
        *self.child.lock().unwrap() = Some(child);

        let this = self.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(message) = serde_json::from_str::<Json>(&line) else { continue };
                this.route(message);
            }
            // stdout closed: fail everything still pending
            let pending: Vec<_> = this.pending.lock().unwrap().drain().collect();
            for (_, tx) in pending {
                let _ = tx.send(Err("codex app-server exited".into()));
            }
        });
        let this = self.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }
                let mut lines = this.stderr_lines.lock().unwrap();
                lines.push(line);
                let len = lines.len();
                if len > 50 {
                    lines.drain(0..len - 50);
                }
            }
        });

        // P2-4: on a failed handshake kill the half-started server — the reader
        // threads hold Arcs, so returning Err without close() leaks the process.
        if let Err(e) = self.call(
            "initialize",
            json!({"clientInfo": {"name": "teamagents", "title": "TeamAgents", "version": "0.1.0"}}),
            60_000,
        ) {
            self.close();
            return Err(e);
        }
        Ok(())
    }

    fn route(self: &Arc<Self>, message: Json) {
        let id = message.get("id").cloned();
        let method = message.get("method").and_then(|v| v.as_str()).map(str::to_string);
        let is_response = message.get("result").is_some() || message.get("error").is_some();
        if let Some(response_id) = id.clone().filter(|_| is_response) {
            let Some(numeric) = response_id.as_u64() else { return };
            let slot = self.pending.lock().unwrap().remove(&numeric);
            if let Some(tx) = slot {
                let result = match message.get("error") {
                    Some(error) if !error.is_null() => Err(format!("{error}")),
                    _ => Ok(message.get("result").cloned().unwrap_or(Json::Null)),
                };
                let _ = tx.send(result);
            }
            return;
        }
        if id.is_some() && method.is_some() {
            let handler = self.on_request.lock().unwrap().clone();
            let this = self.clone();
            let request = message.clone();
            let request_id = message.get("id").cloned().unwrap_or(Json::Null);
            std::thread::spawn(move || {
                // a request handler may block (approval decision): answer when it returns
                let result = match handler {
                    Some(handler) => handler(request),
                    None => json!({}),
                };
                this.respond(request_id, result);
            });
            return;
        }
        if method.is_some() {
            let handler = self.notify.lock().unwrap().clone();
            if let Some(handler) = handler {
                handler(message);
            }
        }
    }

    pub fn call(&self, method: &str, params: Json, timeout_ms: u64) -> Result<Json, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.write(json!({"id": id, "method": method, "params": params}))?;
        match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
            Ok(result) => result,
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!("{method} timed out"))
            }
        }
    }

    fn respond(&self, id: Json, result: Json) {
        let _ = self.write(json!({"id": id, "result": result}));
    }

    fn write(&self, message: Json) -> Result<(), String> {
        let mut guard = self.stdin.lock().unwrap();
        let Some(stdin) = guard.as_mut() else {
            return Err("app-server is not running".into());
        };
        writeln!(stdin, "{message}").map_err(|e| e.to_string())?;
        stdin.flush().map_err(|e| e.to_string())
    }

    pub fn last_stderr(&self) -> Vec<String> {
        self.stderr_lines.lock().unwrap().clone()
    }

    pub fn close(&self) {
        let child = self.child.lock().unwrap().take();
        *self.stdin.lock().unwrap() = None;
        if let Some(mut child) = child {
            #[cfg(unix)]
            {
                // SIGTERM to the whole group via `kill`, then SIGKILL the child
                // (no libc dependency in this crate)
                let pgid = child.id().to_string();
                let _ = Command::new("kill").args(["-TERM", &format!("-{pgid}")]).status();
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub struct CodexOptions {
    pub agent_id: String,
    pub session_id: String,
    pub workdir: std::path::PathBuf,
    pub sandbox: String,
    pub approval_policy: String,
    pub effort: Option<String>,
    pub model: Option<String>,
    pub codex_bin: Option<String>,
    pub codex_home: Option<String>,
    pub env: Vec<(String, String)>,
    pub config_overrides: Vec<(String, Json)>,
}

fn turn_status(name: &str) -> TurnStatus {
    match name {
        "completed" => TurnStatus::Completed,
        "interrupted" => TurnStatus::Cancelled,
        "inProgress" => TurnStatus::Running,
        _ => TurnStatus::Failed,
    }
}

/// How long an app-server approval request may wait for the user before it is
/// declined. ponytail: Python waits forever; the bound keeps a cancelled turn
/// from leaking this thread. The env override exists for the timeout test.
fn approval_wait_timeout() -> Duration {
    std::env::var("TEAMAGENTS_CODEX_APPROVAL_WAIT_S")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(600))
}

pub struct CodexRunner {
    opts: CodexOptions,
    core: Arc<CoreClient>,
    approvals: Arc<ApprovalGate>,
    notify: Arc<Notify>,
    self_ref: Mutex<Weak<CodexRunner>>,
    server: Mutex<Option<Arc<CodexAppServer>>>,
    thread_id: Mutex<Option<String>>,
    states: Mutex<HashMap<String, TurnStatus>>,
    current_turn: Mutex<HashMap<String, String>>,
    turn_done: Mutex<HashMap<String, Arc<(Mutex<bool>, Condvar)>>>,
    progress: Mutex<HashMap<String, Vec<String>>>,
    reported: Mutex<HashMap<String, usize>>,
    approval_waits: Mutex<HashMap<String, Sender<String>>>,
    approval_ids: Mutex<HashMap<String, String>>,
    queued_input: Mutex<HashMap<String, Vec<String>>>,
    buffered_notes: Mutex<HashMap<String, Vec<Json>>>,
    /// Latest `thread/tokenUsage/updated` payload (app-server protocol:
    /// {total: TokenUsageBreakdown, last: ..., modelContextWindow}).
    /// ponytail: session memory only; persist if cross-restart accounting is asked for.
    token_usage: Mutex<Option<Json>>,
    token_usage_calls: AtomicU64,
    effort_fallback_used: AtomicBool,
}

impl CodexRunner {
    pub fn new(opts: CodexOptions, core: Arc<CoreClient>, approvals: Arc<ApprovalGate>, notify: Arc<Notify>) -> Arc<Self> {
        let runner = Arc::new(Self {
            opts,
            core,
            approvals,
            notify,
            self_ref: Mutex::new(Weak::new()),
            server: Mutex::new(None),
            thread_id: Mutex::new(None),
            states: Mutex::new(HashMap::new()),
            current_turn: Mutex::new(HashMap::new()),
            turn_done: Mutex::new(HashMap::new()),
            progress: Mutex::new(HashMap::new()),
            reported: Mutex::new(HashMap::new()),
            approval_waits: Mutex::new(HashMap::new()),
            approval_ids: Mutex::new(HashMap::new()),
            queued_input: Mutex::new(HashMap::new()),
            buffered_notes: Mutex::new(HashMap::new()),
            token_usage: Mutex::new(None),
            token_usage_calls: AtomicU64::new(0),
            effort_fallback_used: AtomicBool::new(false),
        });
        *runner.self_ref.lock().unwrap() = Arc::downgrade(&runner);
        runner
    }

    fn me(&self) -> Option<Arc<CodexRunner>> {
        self.self_ref.lock().unwrap().upgrade()
    }

    /// Same shape as ChatRunner::usage_snapshot; also forwards the window the
    /// app-server reported (the profile's context_window stays authoritative
    /// when both exist — see session.rs::usage_report).
    pub fn usage_snapshot(&self) -> Json {
        let usage = self.token_usage.lock().unwrap().clone().unwrap_or(json!({}));
        codex_usage_snapshot(&usage, self.token_usage_calls.load(Ordering::SeqCst))
    }

    fn ensure_server(&self) -> Result<Arc<CodexAppServer>, String> {
        if let Some(server) = self.server.lock().unwrap().clone() {
            return Ok(server);
        }
        let mut overrides = self.opts.config_overrides.clone();
        if let Some(model) = &self.opts.model {
            overrides.retain(|(k, _)| k != "model");
            overrides.push(("model".into(), json!(model)));
        }
        let server = CodexAppServer::new(
            &self.opts.workdir,
            AppServerOptions {
                codex_bin: self.opts.codex_bin.clone(),
                codex_home: self.opts.codex_home.clone(),
                env: self.opts.env.clone(),
                config_overrides: overrides,
            },
        );
        server.start()?;
        let reply = self.core.call_in_session("get_codex_thread", json!({"agent_id": self.opts.agent_id}))?;
        *self.thread_id.lock().unwrap() = reply.get("thread_id").and_then(|v| v.as_str()).map(str::to_string);
        *self.server.lock().unwrap() = Some(server.clone());
        Ok(server)
    }

    fn ensure_thread(&self, server: &Arc<CodexAppServer>) -> Result<String, String> {
        if let Some(thread) = self.thread_id.lock().unwrap().clone() {
            return Ok(thread);
        }
        let mut params = json!({
            "cwd": self.opts.workdir.to_string_lossy(),
            "sandbox": self.opts.sandbox,
            "approvalPolicy": self.opts.approval_policy,
            "approvalsReviewer": "user",
            "threadSource": "appServer",
        });
        if let Some(model) = &self.opts.model {
            params["model"] = json!(model);
        }
        let result = server.call("thread/start", params, 60_000)?;
        let thread = result
            .get("thread")
            .and_then(|t| t.get("id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| "codex thread/start returned no thread id".to_string())?
            .to_string();
        *self.thread_id.lock().unwrap() = Some(thread.clone());
        // persist the thread before submitting a turn (§10.2)
        self.core.call_in_session("set_codex_thread", json!({"agent_id": self.opts.agent_id, "thread_id": thread}))?;
        Ok(thread)
    }

    fn render_input(&self, view: &Json, wake: &Json) -> String {
        let mut text = crate::chat::render_view(view, wake, Some(&self.opts.workdir.to_string_lossy()));
        let agent = view.get("agent_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let queued = self.queued_input.lock().unwrap().remove(&agent).unwrap_or_default();
        if !queued.is_empty() {
            text.push_str("\n<queued_updates>");
            text.push_str(&queued.join("\n"));
            text.push_str("</queued_updates>");
        }
        text.push_str(
            "\n<codex_member>\nYou are an execution member. Work the assigned task, report progress through your own outputs; the Leader coordinates the team. Do not attempt team-management actions.\n</codex_member>",
        );
        text
    }

    fn on_notification(&self, run_id: &str, message: Json) {
        let expected = self.current_turn.lock().unwrap().get(run_id).cloned();
        match expected {
            // buffer events that arrive before turn/start returns
            None => self
                .buffered_notes
                .lock()
                .unwrap()
                .entry(run_id.to_string())
                .or_default()
                .push(message),
            Some(turn) => self.apply_notification(run_id, &turn, message),
        }
    }

    fn apply_notification(&self, run_id: &str, expected_turn: &str, message: Json) {
        let method = message.get("method").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let params = message.get("params").cloned().unwrap_or(json!({}));
        let turn_id = params
            .get("turn")
            .and_then(|t| t.get("id"))
            .or_else(|| params.get("turnId"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if let Some(turn_id) = &turn_id {
            if turn_id != expected_turn {
                return;
            }
        }
        match method.as_str() {
            "item/agentMessage/delta" => {
                let delta = params.get("delta").and_then(|v| v.as_str()).unwrap_or("").to_string();
                self.progress.lock().unwrap().entry(run_id.to_string()).or_default().push(delta.clone());
                if !delta.is_empty() {
                    self.notify.note_stream_chunk(run_id, &self.opts.agent_id, &delta);
                }
            }
            "item/completed" => {
                let item = params.get("item").cloned().unwrap_or(json!({}));
                let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if item.get("type").and_then(|v| v.as_str()) == Some("agentMessage") && !text.is_empty() {
                    self.progress.lock().unwrap().entry(run_id.to_string()).or_default().push(text.clone());
                    self.notify.note_external_progress(run_id, &text);
                }
            }
            "thread/tokenUsage/updated" => {
                if let Some(usage) = params.get("tokenUsage") {
                    *self.token_usage.lock().unwrap() = Some(usage.clone());
                    self.token_usage_calls.fetch_add(1, Ordering::SeqCst);
                }
            }
            "turn/completed" => {
                let status = params
                    .get("turn")
                    .and_then(|t| t.get("status"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let status = turn_status(status);
                let (pieces, reported) = {
                    let progress = self.progress.lock().unwrap();
                    let pieces = progress.get(run_id).cloned().unwrap_or_default();
                    let reported = self.reported.lock().unwrap().get(run_id).copied().unwrap_or(0);
                    (pieces, reported)
                };
                let new_text = pieces[reported.min(pieces.len())..].join(" ").trim().to_string();
                if !new_text.is_empty() {
                    self.reported.lock().unwrap().insert(run_id.to_string(), pieces.len());
                    self.notify.note_external_progress(run_id, &new_text);
                }
                self.states.lock().unwrap().insert(run_id.to_string(), status);
                if let Some(done) = self.turn_done.lock().unwrap().get(run_id) {
                    let (lock, cv) = &**done;
                    *lock.lock().unwrap() = true;
                    cv.notify_all();
                }
            }
            "error" => {
                let text = params.get("message").and_then(|v| v.as_str()).unwrap_or("").to_string();
                self.progress.lock().unwrap().entry(run_id.to_string()).or_default().push(text);
            }
            _ => {}
        }
    }

    /// Approval/user-input requests coming from the app-server.
    fn on_request(&self, run_id: &str, message: Json) -> Json {
        let method = message.get("method").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let params = message.get("params").cloned().unwrap_or(json!({}));
        if !method.ends_with("requestApproval") {
            if method == "item/tool/requestUserInput" {
                return json!({"answers": []});
            }
            return json!({});
        }
        let kind = method.split('/').nth(1).unwrap_or(&method).to_string();
        let scope = json!({"kind": kind, "request": params});
        let op_hash = operation_hash(&kind, params.get("request").unwrap_or(&params));
        let tool_call_id = params
            .get("itemId")
            .or_else(|| params.get("approvalId"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| teamagents_core::models::new_id("cx"));
        let request = ApprovalRequest {
            approval_id: teamagents_core::models::new_id("appr"),
            session_id: self.opts.session_id.clone(),
            agent_id: self.opts.agent_id.clone(),
            run_id: run_id.to_string(),
            tool_call_id,
            operation_hash: op_hash,
            requested_scope: scope,
            policy_revision: self.approvals.revision(),
            status: ApprovalStatus::Pending,
            created_at: teamagents_core::models::now(),
            decided_at: None,
        };
        let _ = self.core.call_in_session("insert_approval", json!({"approval": request}));
        self.approval_ids.lock().unwrap().insert(run_id.to_string(), request.approval_id.clone());
        self.states.lock().unwrap().insert(run_id.to_string(), TurnStatus::WaitingApproval);
        self.notify.note_external_status(run_id, TurnStatus::WaitingApproval);
        let (tx, rx) = channel();
        self.approval_waits.lock().unwrap().insert(request.approval_id.clone(), tx);
        let decision = match rx.recv_timeout(approval_wait_timeout()) {
            Ok(decision) => decision,
            Err(_) => {
                // the app-server moves on, so nobody can consume this approval
                // anymore: void it in the core too, else the user keeps seeing
                // a PENDING row whose decision is silently dropped
                self.approvals.expire(&request.approval_id);
                "decline".to_string()
            }
        };
        self.approval_waits.lock().unwrap().remove(&request.approval_id);
        self.states.lock().unwrap().insert(run_id.to_string(), TurnStatus::Running);
        self.notify.note_external_status(run_id, TurnStatus::Running);
        json!({"decision": decision})
    }

    fn resolve_approval_for(&self, approval_id: &str, decision: &str) -> bool {
        let wait = self.approval_waits.lock().unwrap().remove(approval_id);
        match wait {
            Some(tx) => {
                let mapped = match decision {
                    "once" => "accept",
                    "session" => "acceptForSession",
                    _ => "decline",
                };
                let _ = tx.send(mapped.to_string());
                true
            }
            None => false,
        }
    }

    fn await_turn_done(&self, run_id: &str, timeout: Option<Duration>) -> bool {
        let Some(done) = self.turn_done.lock().unwrap().get(run_id).cloned() else { return true };
        let (lock, cv) = &*done;
        let mut finished = lock.lock().unwrap();
        match timeout {
            None => {
                while !*finished {
                    finished = cv.wait(finished).unwrap();
                }
                true
            }
            Some(limit) => {
                let deadline = std::time::Instant::now() + limit;
                while !*finished {
                    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                    if remaining.is_zero() {
                        return false;
                    }
                    let (guard, _) = cv.wait_timeout(finished, remaining).unwrap();
                    finished = guard;
                }
                true
            }
        }
    }
}

impl AgentRunner for CodexRunner {
    fn start_or_resume(&self, run: &TurnRun, view: &Json, gateway: &ToolGateway, wake: &Json) -> TurnOutcome {
        let run_id = run.run_id.clone();
        let server = match self.ensure_server() {
            Ok(server) => server,
            Err(e) => return failed(&run_id, format!("CodexError: {e}")),
        };
        let thread_id = match self.ensure_thread(&server) {
            Ok(thread) => thread,
            Err(e) => return failed(&run_id, format!("CodexError: {e}")),
        };
        self.states.lock().unwrap().insert(run_id.clone(), TurnStatus::Running);
        self.turn_done
            .lock()
            .unwrap()
            .insert(run_id.clone(), Arc::new((Mutex::new(false), Condvar::new())));
        self.progress.lock().unwrap().insert(run_id.clone(), vec![]);
        let text = self.render_input(view, wake);
        let mut params = json!({
            "threadId": thread_id,
            "input": [{"type": "text", "text": text}],
            "approvalPolicy": self.opts.approval_policy,
            "approvalsReviewer": "user",
        });
        let effort = self.opts.effort.clone();
        if let Some(value) = &effort {
            params["effort"] = json!(value);
        }
        // handlers live before the turn starts
        let (weak_notify, weak_request) = (self.me(), self.me());
        let notify_run = run_id.clone();
        let request_run = run_id.clone();
        server.set_handlers(
            Arc::new(move |message| {
                if let Some(runner) = &weak_notify {
                    runner.on_notification(&notify_run, message);
                }
            }),
            Arc::new(move |message| match &weak_request {
                Some(runner) => runner.on_request(&request_run, message),
                None => json!({}),
            }),
        );
        let mut result = server.call("turn/start", params.clone(), 120_000);
        if let Err(error) = &result {
            if effort.is_some() && !self.effort_fallback_used.load(Ordering::SeqCst)
                && error.to_lowercase().contains("effort")
            {
                self.effort_fallback_used.store(true, Ordering::SeqCst);
                params["effort"] = json!("max");
                result = server.call("turn/start", params.clone(), 120_000);
            }
        }
        let result = match result {
            Ok(result) => result,
            Err(e) => {
                self.states.lock().unwrap().insert(run_id.clone(), TurnStatus::Failed);
                return failed(&run_id, format!("CodexError: {e}"));
            }
        };
        let turn_id = result
            .get("turn")
            .and_then(|t| t.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        self.current_turn.lock().unwrap().insert(run_id.clone(), turn_id.clone());
        let buffered = self.buffered_notes.lock().unwrap().remove(&run_id).unwrap_or_default();
        for message in buffered {
            self.apply_notification(&run_id, &turn_id, message);
        }
        let _ = self.core.call_in_session(
            "set_run_external_turn",
            json!({"run_id": run_id, "external_turn_id": turn_id}),
        );

        self.await_turn_done(&run_id, None);
        let status = self.states.lock().unwrap().get(&run_id).copied().unwrap_or(TurnStatus::OutcomeUnknown);
        self.turn_done.lock().unwrap().remove(&run_id);
        if status == TurnStatus::WaitingApproval {
            let note = self.approval_ids.lock().unwrap().get(&run_id).cloned();
            return TurnOutcome { status, error: None, note, reply_text: None };
        }
        if status == TurnStatus::Completed {
            if let Some(task_id) = &run.task_id {
                let summary: String = {
                    let pieces = self.progress.lock().unwrap().get(&run_id).cloned().unwrap_or_default();
                    let joined = pieces.join(" ");
                    let chars: Vec<char> = joined.chars().collect();
                    chars[chars.len().saturating_sub(2000)..].iter().collect()
                };
                let receipt = gateway.call(
                    "complete_task",
                    &json!({"task_id": task_id, "summary": summary, "result_refs": []}),
                    &format!("{turn_id}:complete"),
                );
                if !receipt.ok {
                    eprintln!(
                        "completion request rejected for {task_id}: {}",
                        receipt.error.unwrap_or_default()
                    );
                }
            }
        }
        let pieces = self.progress.lock().unwrap().get(&run_id).cloned().unwrap_or_default();
        let joined = pieces.join(" ");
        let chars: Vec<char> = joined.chars().collect();
        let reply: String = chars[chars.len().saturating_sub(4000)..].iter().collect();
        TurnOutcome {
            status,
            error: if status == TurnStatus::Failed { pieces.last().cloned() } else { None },
            note: None,
            reply_text: if reply.is_empty() { None } else { Some(reply) },
        }
    }

    fn request_interrupt(&self, run_id: &str) -> TurnStatus {
        let (server, turn_id, thread_id) = {
            (
                self.server.lock().unwrap().clone(),
                self.current_turn.lock().unwrap().get(run_id).cloned(),
                self.thread_id.lock().unwrap().clone(),
            )
        };
        let (Some(server), Some(turn_id), Some(thread_id)) = (server, turn_id, thread_id) else {
            self.states.lock().unwrap().insert(run_id.to_string(), TurnStatus::Cancelled);
            return TurnStatus::Cancelled;
        };
        let _ = server.call(
            "turn/interrupt",
            json!({"threadId": thread_id, "turnId": turn_id}),
            30_000,
        );
        // cancellation is confirmed only when the turn reports a terminal state
        if !self.await_turn_done(run_id, Some(Duration::from_secs(30))) {
            self.states.lock().unwrap().insert(run_id.to_string(), TurnStatus::OutcomeUnknown);
        }
        let status = self.states.lock().unwrap().get(run_id).copied().unwrap_or(TurnStatus::OutcomeUnknown);
        match status {
            TurnStatus::Cancelled | TurnStatus::Completed | TurnStatus::Failed => status,
            _ => TurnStatus::OutcomeUnknown,
        }
    }

    fn query_state(&self, run_id: &str) -> Option<TurnStatus> {
        self.states.lock().unwrap().get(run_id).copied()
    }

    fn deliver_mid_turn(&self, run_id: &str, items: Vec<Json>) {
        let texts: Vec<String> = items
            .iter()
            .map(|i| {
                format!(
                    "<inbox from=\"{}\" kind=\"{}\">{}</inbox>",
                    i.get("from").and_then(|v| v.as_str()).unwrap_or(""),
                    i.get("kind").and_then(|v| v.as_str()).unwrap_or(""),
                    i.get("payload").cloned().unwrap_or(json!({}))
                )
            })
            .collect();
        self.queued_input
            .lock()
            .unwrap()
            .entry(self.opts.agent_id.clone())
            .or_default()
            .extend(texts.clone());
        // a live turn gets nudged via turn/steer when possible
        let turn = self.current_turn.lock().unwrap().get(run_id).cloned();
        let (server, thread) = (self.server.lock().unwrap().clone(), self.thread_id.lock().unwrap().clone());
        if let (Some(turn), Some(server), Some(thread)) = (turn, server, thread) {
            let input: Vec<Json> = texts.iter().map(|t| json!({"type": "text", "text": t})).collect();
            std::thread::spawn(move || {
                let _ = server.call("turn/steer", json!({"threadId": thread, "turnId": turn, "input": input}), 30_000);
            });
        }
    }

    /// A codex turn survives our restart; query the live thread state.
    fn reconcile(&self, run: &TurnRun) -> Option<TurnStatus> {
        let server = self.server.lock().unwrap().clone()?;
        let thread = self.thread_id.lock().unwrap().clone()?;
        run.external_turn_id.as_ref()?;
        // codex.py: no `thread/status` method exists (it is a notification);
        // the history is read instead.
        let result = server
            .call("thread/read", json!({"threadId": thread, "includeTurns": true}), 30_000)
            .ok()?;
        let turns = result
            .get("thread")
            .and_then(|t| t.get("turns"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let last = turns.last()?;
        // a live turn is unverifiable after our restart, same as an unknown status
        Some(match last.get("status").and_then(|v| v.as_str()).unwrap_or("") {
            "completed" => TurnStatus::Completed,
            "interrupted" => TurnStatus::Cancelled,
            "failed" => TurnStatus::Failed,
            _ => TurnStatus::OutcomeUnknown,
        })
    }

    fn resolve_approval(&self, approval_id: &str, decision: &str) -> bool {
        self.resolve_approval_for(approval_id, decision)
    }

    fn close(&self) {
        if let Some(server) = self.server.lock().unwrap().take() {
            server.close();
        }
    }
}

fn failed(run_id: &str, error: String) -> TurnOutcome {
    let _ = run_id;
    TurnOutcome { status: TurnStatus::Failed, error: Some(error), note: None, reply_text: None }
}

/// codex app-server `thread/tokenUsage/updated` payload -> the shared
/// usage shape. `total` is the thread-cumulative breakdown, `last` the latest
/// turn; last.totalTokens approximates the current context fill.
fn codex_usage_snapshot(usage: &Json, calls: u64) -> Json {
    let num = |section: &str, key: &str| usage.get(section).and_then(|s| s.get(key)).and_then(Json::as_u64).unwrap_or(0);
    json!({
        "calls": calls,
        "prompt_tokens": num("total", "inputTokens"),
        "completion_tokens": num("total", "outputTokens"),
        "total_tokens": num("total", "totalTokens"),
        "last_prompt_tokens": num("last", "totalTokens"),
        "codex_context_window": usage.get("modelContextWindow").and_then(Json::as_u64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_usage_snapshot_maps_the_app_server_breakdown() {
        let usage = json!({
            "total": {"inputTokens": 1000, "outputTokens": 200, "reasoningOutputTokens": 50,
                      "cachedInputTokens": 100, "totalTokens": 1250},
            "last": {"inputTokens": 300, "outputTokens": 80, "reasoningOutputTokens": 20,
                     "cachedInputTokens": 0, "totalTokens": 400},
            "modelContextWindow": 272000,
        });
        let snap = codex_usage_snapshot(&usage, 3);
        assert_eq!(snap["calls"], 3);
        assert_eq!(snap["prompt_tokens"], 1000);
        assert_eq!(snap["completion_tokens"], 200);
        assert_eq!(snap["total_tokens"], 1250);
        assert_eq!(snap["last_prompt_tokens"], 400);
        assert_eq!(snap["codex_context_window"], 272000);
        // no notification yet -> zeros, null window
        let empty = codex_usage_snapshot(&json!({}), 0);
        assert_eq!(empty["total_tokens"], 0);
        assert_eq!(empty["codex_context_window"], Json::Null);
    }
}
