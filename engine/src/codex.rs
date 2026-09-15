//! Codex execution member backend: one `codex app-server` JSON-RPC
//! process, one thread per member, approvals parked in the core.

use crate::core_client::CoreClient;
use crate::gateway::{operation_hash, ApprovalGate, ToolGateway};
use crate::runtime::{AgentRunner, Notify};
use serde_json::{json, Value as Json};
use std::collections::{HashMap, HashSet};
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
    on_exit: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    next_id: AtomicU64,
    stderr_lines: Mutex<Vec<String>>,
}

/// argv for `codex app-server`. Codex config profiles are layered as `-c`
/// overrides (see `codex_profile_overrides`): the CLI rejects `--profile` for
/// this subcommand.
fn app_server_args(opts: &AppServerOptions) -> Vec<String> {
    let mut args: Vec<String> = vec!["app-server".into()];
    for (key, value) in &opts.config_overrides {
        args.push("-c".into());
        args.push(match value {
            Json::String(s) => format!("{key}={s}"),
            other => format!("{key}={other}"),
        });
    }
    args
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
            on_exit: Mutex::new(None),
            next_id: AtomicU64::new(1),
            stderr_lines: Mutex::new(vec![]),
        })
    }

    pub fn set_handlers(&self, notify: NotifyHandler, on_request: RequestHandler) {
        *self.notify.lock().unwrap() = Some(notify);
        *self.on_request.lock().unwrap() = Some(on_request);
    }

    /// Fired once by the reader thread when the server's stdout closes.
    pub fn set_on_exit(&self, on_exit: Arc<dyn Fn() + Send + Sync>) {
        *self.on_exit.lock().unwrap() = Some(on_exit);
    }

    pub fn start(self: &Arc<Self>) -> Result<(), String> {
        let args = app_server_args(&self.opts);
        let mut command = Command::new(self.opts.codex_bin.clone().unwrap_or_else(|| "codex".into()));
        command
            .args(&args)
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // own process group (start_new_session + killpg semantics): the shell
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
            // stdout closed: fail everything still pending, and say why when the
            // server explained itself on stderr (a bad flag/profile is otherwise
            // indistinguishable from a crash)
            let detail = this.stderr_tail();
            let reason = if detail.is_empty() {
                "codex app-server exited".to_string()
            } else {
                format!("codex app-server exited: {detail}")
            };
            let pending: Vec<_> = this.pending.lock().unwrap().drain().collect();
            for (_, tx) in pending {
                let _ = tx.send(Err(reason.clone()));
            }
            // no turn/completed is coming either: release the driving threads
            if let Some(on_exit) = this.on_exit.lock().unwrap().clone() {
                on_exit();
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
            json!({"clientInfo": {"name": "teamagents", "title": "TeamAgents", "version": env!("CARGO_PKG_VERSION")}}),
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
        if let Err(e) = self.write(json!({"id": id, "method": method, "params": params})) {
            // a dead server never answers: drop the entry we just parked
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
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

    /// Last few stderr lines, joined for an error message.
    fn stderr_tail(&self) -> String {
        let lines = self.stderr_lines.lock().unwrap();
        let tail: Vec<String> = lines.iter().rev().take(3).rev().cloned().collect();
        tail.join(" | ")
    }

    pub fn close(&self) {
        let child = self.child.lock().unwrap().take();
        *self.stdin.lock().unwrap() = None;
        if let Some(mut child) = child {
            #[cfg(unix)]
            {
                // The whole group goes together: the shell commands an app-server
                // spawns are its children, and killing only the direct child
                // leaves them running. `kill` as the shell's builtin (POSIX
                // requires it, /bin/sh always has it) instead of an external
                // binary that minimal images may not ship.
                let pgid = child.id();
                let group_kill = |signal: &str| {
                    let _ = Command::new("/bin/sh").args(["-c", &format!("kill -{signal} -{pgid}")]).status();
                };
                group_kill("TERM");
                std::thread::sleep(Duration::from_millis(100));
                // a grandchild that ignores TERM still has to go
                group_kill("KILL");
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[derive(Default)]
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
/// declined. ponytail: the bound keeps a cancelled turn from leaking this
/// thread (an unbounded wait would park it forever). The env override exists
/// for the timeout test.
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
    /// The member's own text for a run, assembled from stream deltas and
    /// completed agent messages. Deltas are contiguous pieces (concatenate,
    /// never join with a separator) and the completed item repeats the same
    /// text, so it is appended only when it is not already the tail.
    agent_text: Mutex<HashMap<String, String>>,
    reported: Mutex<HashMap<String, usize>>,
    approval_waits: Mutex<HashMap<String, Sender<String>>>,
    approval_ids: Mutex<HashMap<String, String>>,
    queued_input: Mutex<HashMap<String, Vec<String>>>,
    /// run_id → event_ids already injected (steer/queued), the same guard as
    /// ChatRunner's checkpoint.input_events: a re-drained push is not delivered twice
    delivered: Mutex<HashMap<String, HashSet<String>>>,
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
            agent_text: Mutex::new(HashMap::new()),
            reported: Mutex::new(HashMap::new()),
            approval_waits: Mutex::new(HashMap::new()),
            approval_ids: Mutex::new(HashMap::new()),
            queued_input: Mutex::new(HashMap::new()),
            delivered: Mutex::new(HashMap::new()),
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
        let thread = reply.get("thread_id").and_then(|v| v.as_str()).map(str::to_string);
        if let Some(id) = &thread {
            let mut params = json!({"threadId": id, "cwd": self.opts.workdir.to_string_lossy(),
                "approvalPolicy": self.opts.approval_policy, "sandbox": self.opts.sandbox});
            if let Some(model) = &self.opts.model { params["model"] = json!(model); }
            if let Some((_, provider)) = self.opts.config_overrides.iter().rev().find(|(key, _)| key == "model_provider") {
                params["modelProvider"] = provider.clone();
            }
            if let Err(e) = server.call("thread/resume", params, 60_000) { server.close(); return Err(e); }
        }
        *self.thread_id.lock().unwrap() = thread;
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
            None => {
                // buffer events that arrive before turn/start returns — but a
                // note for an already-finished run is late: nothing would ever
                // drain it (states only gains entries, never loses them)
                let finished = self.states.lock().unwrap().get(run_id).is_some_and(|s| s.is_terminal());
                if !finished {
                    self.buffered_notes.lock().unwrap().entry(run_id.to_string()).or_default().push(message);
                }
            }
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
                if !delta.is_empty() {
                    self.agent_text.lock().unwrap().entry(run_id.to_string()).or_default().push_str(&delta);
                    self.notify.note_stream_chunk(run_id, &self.opts.agent_id, &delta);
                }
            }
            "item/completed" => {
                let item = params.get("item").cloned().unwrap_or(json!({}));
                let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if item.get("type").and_then(|v| v.as_str()) == Some("agentMessage") && !text.is_empty() {
                    let mut texts = self.agent_text.lock().unwrap();
                    let buffer = texts.entry(run_id.to_string()).or_default();
                    // the deltas usually spell this text already
                    if !buffer.ends_with(&text) {
                        if !buffer.is_empty() {
                            buffer.push('\n');
                        }
                        buffer.push_str(&text);
                    }
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
                let (text, reported) = {
                    let texts = self.agent_text.lock().unwrap();
                    let text = texts.get(run_id).cloned().unwrap_or_default();
                    let reported = self.reported.lock().unwrap().get(run_id).copied().unwrap_or(0);
                    (text, reported)
                };
                let length = text.chars().count();
                let new_text: String = text.chars().skip(reported.min(length)).collect();
                if !new_text.trim().is_empty() {
                    self.reported.lock().unwrap().insert(run_id.to_string(), length);
                    self.notify.note_external_progress(run_id, &new_text);
                }
                self.states.lock().unwrap().insert(run_id.to_string(), status);
                let done = self.turn_done.lock().unwrap().get(run_id).cloned();
                match done {
                    Some(done) => {
                        let (lock, cv) = &*done;
                        *lock.lock().unwrap() = true;
                        cv.notify_all();
                    }
                    // no driving thread (a turn resumed after approval finishes
                    // here): nobody else will release the per-run maps
                    None => self.clear_run_state(run_id),
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
        // The wire payload carries per-call unique ids (threadId/turnId/
        // itemId/startedAtMs), so the operation hash projects only the
        // discriminating fields — hashing the raw params makes every call
        // unique and a session grant could never match. Unknown kinds hash
        // the whole payload: fail-closed until their projection is added.
        // ponytail: commandExecution omits params.kind (writeStdin) and
        // networkApprovalContext — unreachable under the fixed
        // workspace-write + on-request deployment; add them to the
        // projection when managed-network/unified-exec is enabled.
        // fileChange has no discriminating field on the wire (grantRoot is
        // UNSTABLE and usually absent), so it stays fail-closed rather than
        // bucket every file change into one grant.
        let operation = match kind.as_str() {
            "commandExecution" => json!({"command": params.get("command"), "cwd": params.get("cwd")}),
            "permissions" => json!({"permissions": params.get("permissions"), "cwd": params.get("cwd")}),
            _ => params.clone(),
        };
        let op_hash = operation_hash(&kind, &operation);
        // Session-level reuse lives in the core only: the app-server never
        // receives acceptForSession, so it re-asks every call and an active
        // per-operation grant answers without parking a new PENDING row.
        if self.approvals.session_grant_active(&op_hash) {
            // a terminal run's in-flight request declines: the turn is being
            // torn down, the operation must not run under a stale grant
            let terminal = self.states.lock().unwrap().get(run_id).is_some_and(|s| s.is_terminal());
            return json!({"decision": if terminal { "decline" } else { "accept" }});
        }
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
            operation_hash: op_hash.clone(),
            requested_scope: scope,
            policy_revision: self.approvals.revision(),
            status: ApprovalStatus::Pending,
            created_at: teamagents_core::models::now(),
            decided_at: None,
        };
        if let Err(e) = self.core.call_in_session("insert_approval", json!({"approval": request})) {
            eprintln!("approval {} was not recorded in the core: {e}", request.approval_id);
        }
        self.approval_ids.lock().unwrap().insert(run_id.to_string(), request.approval_id.clone());
        self.states.lock().unwrap().insert(run_id.to_string(), TurnStatus::WaitingApproval);
        self.notify.note_external_status(run_id, TurnStatus::WaitingApproval);
        let (tx, rx) = channel();
        self.approval_waits.lock().unwrap().insert(request.approval_id.clone(), tx);
        // 决定可能比 waiter 注册更早到达（用户/自动化在 PENDING 行出现的瞬间就拍板）：
        // 注册后回读一次行状态，已决定就直接采用 —— 否则这个 waiter 会干等到
        // approval_wait_timeout 后把决定丢掉、回一个 decline 给 app-server。
        let decision = match self.decided_before_waiting(&request.approval_id) {
            Some(decision) => decision,
            None => match rx.recv_timeout(approval_wait_timeout()) {
                Ok(decision) => decision,
                Err(_) => {
                // ponytail: this waiter thread is not released on server
                // death, it sits in recv_timeout up to approval_wait_timeout;
                // upgrade path = server_exited broadcasts a decline through
                // the approval_waits registry, add when the linger matters
                // the app-server moves on, so nobody can consume this approval
                // anymore: void it in the core too, else the user keeps seeing
                // a PENDING row whose decision is silently dropped
                self.approvals.expire(&request.approval_id);
                "decline".to_string()
                }
            },
        };
        self.approval_waits.lock().unwrap().remove(&request.approval_id);
        // do not resurrect a run server_exited already failed while this
        // waiter was parked: a terminal status wins over the Running restore
        let mut states = self.states.lock().unwrap();
        if !matches!(states.get(run_id), Some(s) if s.is_terminal()) {
            states.insert(run_id.to_string(), TurnStatus::Running);
            drop(states);
            self.notify.note_external_status(run_id, TurnStatus::Running);
        }
        if decision == "session" {
            self.approvals.note_session_grant(&op_hash, request.policy_revision);
            // the grant authorizes this operation for everyone: siblings
            // parked on the identical op_hash get the answer too
            if self.approvals.session_grant_active(&op_hash) {
                self.release_grant_siblings(&request.approval_id, &op_hash);
            }
        }
        // "session" maps to a single-op accept on the wire: the grant above,
        // not the app-server's own session cache, covers the next identical
        // call — the app-server cache is not bound to one operation_hash.
        let mapped = match decision.as_str() {
            "once" | "session" => "accept",
            _ => "decline",
        };
        json!({"decision": mapped})
    }

    /// Release approval waiters parked on the same operation a "session"
    /// grant just authorized: their now-moot PENDING rows are expired and
    /// their waiters answered. Rows parked by the ToolGateway path or by
    /// another runner instance have no waiter in this map and keep their own
    /// decision path. A released sibling's row lands EXPIRED although the
    /// operation ran: the audit trail is the APPROVED_SESSION row plus the
    /// session_approval_cache row.
    /// ponytail: a sibling whose insert_approval lands after the state
    /// snapshot is missed and declines on its own timeout — RT-06 finalizes
    /// the row; rescan instead of snapshot if this ever bites.
    fn release_grant_siblings(&self, decided_id: &str, op_hash: &str) {
        let Ok(state) = self.core.state_brief() else { return };
        let pending = state.get("pending_approvals").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        for row in pending {
            let aid = row.get("approval_id").and_then(|v| v.as_str()).unwrap_or("");
            if aid == decided_id || row.get("operation_hash").and_then(|v| v.as_str()) != Some(op_hash) {
                continue;
            }
            let wait = self.approval_waits.lock().unwrap().remove(aid);
            if let Some(tx) = wait {
                self.approvals.expire(aid);
                // "once" so the sibling maps to accept without re-noting the grant
                let _ = tx.send("once".to_string());
            }
        }
    }

    fn resolve_approval_for(&self, approval_id: &str, decision: &str) -> bool {
        let wait = self.approval_waits.lock().unwrap().remove(approval_id);
        match wait {
            Some(tx) => {
                // the waiter receives the core vocabulary and maps it at the
                // reply point, where the operation hash is in scope
                let _ = tx.send(decision.to_string());
                true
            }
            None => false,
        }
    }

    /// Re-reads the approval row right after the waiter was registered: a
    /// decision that landed in that window is returned here instead of being
    /// waited for (and lost until the wait timeout).
    fn decided_before_waiting(&self, approval_id: &str) -> Option<String> {
        let reply = self.core.call_in_session("get_approval", json!({"approval_id": approval_id})).ok()?;
        match reply.get("approval")?.get("status")?.as_str()? {
            "APPROVED_ONCE" => Some("once".into()),
            "APPROVED_SESSION" => Some("session".into()),
            "DENIED" => Some("deny".into()),
            _ => None,
        }
    }

    /// The app-server's stdout closed mid-turn: no turn/completed is coming,
    /// so fail every live run and wake its driver to finalize with Failed.
    /// A run without a driver (parked on an approval) has nobody left to
    /// finalize it: land the terminal status in core and void the approval
    /// nobody can answer anymore, else the core row stays non-terminal and
    /// the PENDING approval swallows the user's decision (round 5, F2).
    fn server_exited(&self) {
        // drop the dead handle so the next ensure_server respawns instead of
        // reusing a corpse whose every write is a Broken pipe
        self.server.lock().unwrap().take();
        let live: Vec<String> = self
            .states
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, status)| !status.is_terminal())
            .map(|(run_id, _)| run_id.clone())
            .collect();
        for run_id in live {
            self.progress.lock().unwrap().entry(run_id.clone()).or_default().push("codex app-server exited".into());
            self.states.lock().unwrap().insert(run_id.clone(), TurnStatus::Failed);
            if let Some(done) = self.turn_done.lock().unwrap().get(&run_id).cloned() {
                let (lock, cv) = &*done;
                *lock.lock().unwrap() = true;
                cv.notify_all();
            } else {
                self.notify.note_external_status(&run_id, TurnStatus::Failed);
                if let Some(approval_id) = self.approval_ids.lock().unwrap().get(&run_id).cloned() {
                    self.approvals.expire(&approval_id);
                }
            }
        }
    }

    /// Drop per-run bookkeeping once the turn no longer needs it.
    /// ponytail: a run parked on WaitingApproval keeps its maps until the
    /// resumed turn ends; the approval_ids note is read from them.
    fn clear_run_state(&self, run_id: &str) {
        self.turn_done.lock().unwrap().remove(run_id);
        self.current_turn.lock().unwrap().remove(run_id);
        self.progress.lock().unwrap().remove(run_id);
        self.agent_text.lock().unwrap().remove(run_id);
        self.reported.lock().unwrap().remove(run_id);
        self.approval_ids.lock().unwrap().remove(run_id);
        self.delivered.lock().unwrap().remove(run_id);
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
        if let Some(model) = &self.opts.model { params["model"] = json!(model); }
        if let Some(value) = &effort {
            params["effort"] = json!(value);
        }
        // handlers live before the turn starts
        let (weak_notify, weak_request) = (self.me(), self.me());
        let weak_exit = self.me();
        server.set_on_exit(Arc::new(move || {
            if let Some(runner) = &weak_exit {
                runner.server_exited();
            }
        }));
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
                self.clear_run_state(&run_id);
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
                    let joined = self.agent_text.lock().unwrap().get(&run_id).cloned().unwrap_or_default();
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
        let joined = self.agent_text.lock().unwrap().get(&run_id).cloned().unwrap_or_default();
        let chars: Vec<char> = joined.chars().collect();
        let reply: String = chars[chars.len().saturating_sub(4000)..].iter().collect();
        let outcome = TurnOutcome {
            status,
            error: if status == TurnStatus::Failed { pieces.last().cloned() } else { None },
            note: None,
            reply_text: if reply.is_empty() { None } else { Some(reply) },
        };
        self.clear_run_state(&run_id);
        outcome
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
            // a turn that finished naturally while the cancel was in flight
            // keeps its terminal status; rewriting it to Cancelled would
            // finalize a succeeded turn as cancelled
            let existing = self.states.lock().unwrap().get(run_id).copied();
            return match existing {
                Some(status) if status.is_terminal() => status,
                _ => {
                    self.states.lock().unwrap().insert(run_id.to_string(), TurnStatus::Cancelled);
                    TurnStatus::Cancelled
                }
            };
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
        let fresh: Vec<Json> = {
            let mut delivered = self.delivered.lock().unwrap();
            let seen = delivered.entry(run_id.to_string()).or_default();
            items
                .into_iter()
                .filter(|i| i.get("event_id").and_then(|v| v.as_str()).map(|id| seen.insert(id.to_string())).unwrap_or(true))
                .collect()
        };
        if fresh.is_empty() {
            return;
        }
        let texts: Vec<String> = fresh
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
        // no `thread/status` method exists (it is a notification);
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

    fn has_approval_waiter(&self) -> bool {
        true
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

    fn test_runner(tag: &str) -> Arc<CodexRunner> {
        let core = CoreClient::open(":memory:", tag).expect("core");
        let approvals = ApprovalGate::new(core.clone(), crate::gateway::PermissionPolicy::default());
        let notify = Notify::new(core.clone());
        // session_id matches the core session, as session.rs wires it in production
        let opts = CodexOptions { session_id: tag.to_string(), ..Default::default() };
        CodexRunner::new(opts, core, approvals, notify)
    }

    #[test]
    fn interrupt_after_natural_completion_keeps_the_terminal_status() {
        let runner = test_runner("s-int-done");
        // no live server/turn: the interrupt takes the early-return branch
        runner.states.lock().unwrap().insert("r1".into(), TurnStatus::Completed);
        assert_eq!(runner.request_interrupt("r1"), TurnStatus::Completed);
        assert_eq!(runner.query_state("r1"), Some(TurnStatus::Completed), "not rewritten to Cancelled");
        runner.states.lock().unwrap().insert("r2".into(), TurnStatus::Running);
        assert_eq!(runner.request_interrupt("r2"), TurnStatus::Cancelled, "live turn still cancels");
    }

    #[test]
    fn streamed_deltas_and_the_completed_item_make_one_clean_reply() {
        // deltas are contiguous pieces; the completed item repeats them
        let buffer = "I'll start by reproducing";
        let item = "I'll start by reproducing the failure.";
        let mut assembled = buffer.to_string();
        if !assembled.ends_with(item) {
            assembled.push('\n');
            assembled.push_str(item);
        }
        assert_eq!(assembled, "I'll start by reproducing\nI'll start by reproducing the failure.");
        // a second message appends on its own line, never as word soup
        let second = "Done.";
        if !assembled.ends_with(second) {
            assembled.push('\n');
            assembled.push_str(second);
        }
        assert!(assembled.ends_with("failure.\nDone."), "{assembled}");
        assert!(!assembled.contains("I 'll"), "no space-joined deltas: {assembled}");
    }

    #[test]
    fn app_server_args_reach_the_subcommand() {
        let with_profile = AppServerOptions {
            codex_bin: None,
            codex_home: None,
            env: vec![],
            config_overrides: vec![("model".into(), json!("deepseek-flash"))],
        };
        assert_eq!(
            app_server_args(&with_profile),
            vec!["app-server", "-c", "model=deepseek-flash"],
            "the subcommand comes first; profiles travel as -c overrides"
        );
    }

    #[test]
    fn turn_completed_without_a_driving_thread_releases_run_state() {
        let runner = test_runner("s-parked");
        // parked-after-approval shape: the driver returned, only maps remain
        runner.current_turn.lock().unwrap().insert("r1".into(), "t1".into());
        runner.progress.lock().unwrap().insert("r1".into(), vec!["work".into()]);
        runner.approval_ids.lock().unwrap().insert("r1".into(), "appr-1".into());
        runner.on_notification("r1", json!({"method": "turn/completed", "params": {"turn": {"id": "t1", "status": "completed"}}}));
        assert_eq!(runner.query_state("r1"), Some(TurnStatus::Completed));
        assert!(!runner.current_turn.lock().unwrap().contains_key("r1"));
        assert!(!runner.progress.lock().unwrap().contains_key("r1"));
        assert!(!runner.approval_ids.lock().unwrap().contains_key("r1"));
    }

    #[test]
    fn turn_completed_with_a_driving_thread_keeps_run_state() {
        let runner = test_runner("s-driven");
        runner.current_turn.lock().unwrap().insert("r1".into(), "t1".into());
        runner.progress.lock().unwrap().insert("r1".into(), vec!["work".into()]);
        runner.approval_ids.lock().unwrap().insert("r1".into(), "appr-1".into());
        runner
            .turn_done
            .lock()
            .unwrap()
            .insert("r1".into(), Arc::new((Mutex::new(false), Condvar::new())));
        runner.on_notification("r1", json!({"method": "turn/completed", "params": {"turn": {"id": "t1", "status": "completed"}}}));
        assert!(runner.progress.lock().unwrap().contains_key("r1"), "the driver still reads it");
        assert!(runner.approval_ids.lock().unwrap().contains_key("r1"));
        assert!(runner.turn_done.lock().unwrap().contains_key("r1"), "signalled, entry kept for the driver");
    }

    #[test]
    fn late_notifications_for_a_finished_run_are_not_buffered() {
        let runner = test_runner("s-late-note");
        runner.states.lock().unwrap().insert("r1".into(), TurnStatus::Completed);
        runner.on_notification("r1", json!({"method": "thread/tokenUsage/updated", "params": {}}));
        assert!(!runner.buffered_notes.lock().unwrap().contains_key("r1"), "a late note is dropped, not buffered");
        // an early note for a live run still buffers until turn/start returns
        runner.states.lock().unwrap().insert("r2".into(), TurnStatus::Running);
        runner.on_notification("r2", json!({"method": "thread/tokenUsage/updated", "params": {}}));
        assert_eq!(runner.buffered_notes.lock().unwrap().get("r2").map(Vec::len), Some(1));
    }

    #[test]
    fn mid_turn_delivery_dedupes_by_event_id_and_clears_with_the_run() {
        let core = CoreClient::open(":memory:", "s-dedup").expect("core");
        let approvals = ApprovalGate::new(core.clone(), crate::gateway::PermissionPolicy::default());
        let notify = Notify::new(core.clone());
        let runner = CodexRunner::new(CodexOptions::default(), core, approvals, notify);
        let item = |id: &str| json!({"event_id": id, "from": "user", "kind": "user_message", "payload": {"text": "hi"}});
        let queued = |runner: &Arc<CodexRunner>| {
            runner.queued_input.lock().unwrap().get(&runner.opts.agent_id).cloned().unwrap_or_default().len()
        };
        runner.deliver_mid_turn("r1", vec![item("e1")]);
        runner.deliver_mid_turn("r1", vec![item("e1"), item("e2")]);
        assert_eq!(queued(&runner), 2, "e1 injected once, e2 once");
        runner.clear_run_state("r1");
        assert!(!runner.current_turn.lock().unwrap().contains_key("r1"));
        assert!(!runner.progress.lock().unwrap().contains_key("r1"));
        assert!(!runner.reported.lock().unwrap().contains_key("r1"));
        assert!(!runner.approval_ids.lock().unwrap().contains_key("r1"));
        assert!(!runner.delivered.lock().unwrap().contains_key("r1"));
        runner.deliver_mid_turn("r1", vec![item("e2")]);
        assert_eq!(queued(&runner), 3, "dedup state ended with the run, e2 is fresh again");
    }

    /// Round 5 (F2): the app-server dying while a run is parked on an
    /// approval — no driving thread, no turn_done waiter — must land Failed
    /// in the core and void the pending approval, not just mark the map.
    #[test]
    fn server_exit_fails_a_driverless_parked_run_in_core_and_expires_its_approval() {
        let core = CoreClient::open(":memory:", "s-parked-die").expect("core");
        core.call("create_session", json!({"session_id": "s-parked-die", "cwd": "/tmp"})).expect("create");
        core.call("set_catalog", json!({"session_id": "s-parked-die", "catalog": {
            "models": {"m": {"provider": "openai", "protocol": "openai", "model": "test"}},
            "tools": {}, "skills_paths": [], "instruction_files": [],
        }})).expect("catalog");
        core.call("save_spec", json!({"session_id": "s-parked-die", "spec": {
            "leader_id": "leader",
            "agents": [
                {"id": "leader", "name": "leader", "role": "leader", "runtime_kind": "deepagents", "model_profile": "m"},
                {"id": "cx", "name": "cx", "role": "worker", "runtime_kind": "codex", "model_profile": "m"},
            ],
            "channels": [{"source": "leader", "targets": ["cx"], "mode": "message"}],
        }})).expect("spec");
        // a real run row for cx, as the runtime would have created it
        let action: teamagents_core::models::TeamAction = serde_json::from_value(json!({
            "action_id": "m1", "session_id": "s-parked-die", "actor_id": "leader",
            "kind": "send_message", "payload": {"target": "cx", "text": "hi"},
        }))
        .expect("action");
        core.submit(&action).expect("submit");
        let state = core.state_brief().expect("state");
        let runs: Vec<TurnRun> = serde_json::from_value(state.get("runs").cloned().unwrap_or(Json::Null)).unwrap_or_default();
        let run_id = runs.iter().find(|r| r.agent_id == "cx").expect("a run for cx").run_id.clone();
        core.call_in_session("insert_approval", json!({"approval": {
            "approval_id": "appr-parked", "session_id": "s-parked-die", "agent_id": "cx",
            "run_id": run_id, "tool_call_id": "call-1", "operation_hash": "h",
            "requested_scope": {}, "policy_revision": 1,
        }}))
        .expect("approval");

        let approvals = ApprovalGate::new(core.clone(), crate::gateway::PermissionPolicy::default());
        let notify = Notify::new(core.clone());
        let runner = CodexRunner::new(CodexOptions::default(), core.clone(), approvals, notify);
        // parked shape: live status + approval note, no driving thread
        runner.states.lock().unwrap().insert(run_id.clone(), TurnStatus::WaitingApproval);
        runner.approval_ids.lock().unwrap().insert(run_id.clone(), "appr-parked".into());
        runner.server_exited();

        assert_eq!(runner.query_state(&run_id), Some(TurnStatus::Failed));
        let state = core.state_brief().expect("state");
        let runs: Vec<TurnRun> = serde_json::from_value(state.get("runs").cloned().unwrap_or(Json::Null)).unwrap_or_default();
        assert_eq!(
            runs.iter().find(|r| r.run_id == run_id).map(|r| r.status),
            Some(TurnStatus::Failed),
            "the core row is finalized, not left non-terminal"
        );
        let pending = state.get("pending_approvals").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        assert!(pending.is_empty(), "the dead approval is expired: {pending:?}");
    }

    /// Round 6: an approval waiter timing out after server_exited landed the
    /// terminal state must not resurrect the run to Running; a live run keeps
    /// the restore semantics.
    #[test]
    fn approval_waiter_timeout_does_not_overwrite_a_terminal_state() {
        std::env::set_var("TEAMAGENTS_CODEX_APPROVAL_WAIT_S", "1");
        let runner = test_runner("s-waiter-term");
        let r = runner.clone();
        let waiter = std::thread::spawn(move || {
            r.on_request("r1", json!({"method": "item/commandExecution/requestApproval", "params": {"itemId": "i1"}}))
        });
        // wait until the handler is parked, then land Failed as server_exited would
        for _ in 0..100 {
            if runner.query_state("r1") == Some(TurnStatus::WaitingApproval) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(runner.query_state("r1"), Some(TurnStatus::WaitingApproval), "waiter parked");
        runner.states.lock().unwrap().insert("r1".into(), TurnStatus::Failed);
        let reply = waiter.join().expect("waiter joined");
        assert_eq!(reply, json!({"decision": "decline"}), "a timeout still declines");
        assert_eq!(runner.query_state("r1"), Some(TurnStatus::Failed), "terminal state not resurrected");
    }

    #[test]
    fn approval_waiter_timeout_restores_running_for_a_live_run() {
        std::env::set_var("TEAMAGENTS_CODEX_APPROVAL_WAIT_S", "1");
        let runner = test_runner("s-waiter-live");
        let reply = runner.on_request("r1", json!({"method": "item/commandExecution/requestApproval", "params": {"itemId": "i1"}}));
        assert_eq!(reply, json!({"decision": "decline"}));
        assert_eq!(runner.query_state("r1"), Some(TurnStatus::Running), "live run resumes Running after the timeout");
    }

    /// 决定比 waiter 注册更早到达（用户/自动化在 PENDING 行出现的瞬间拍板）时，
    /// 不能干等到 approval_wait_timeout 再丢掉决定：注册后回读一次行状态即可。
    #[test]
    fn an_already_decided_approval_is_read_instead_of_waited_for() {
        let runner = test_runner("s-early-read");
        runner
            .core
            .call("create_session", json!({"session_id": "s-early-read", "cwd": "/tmp"}))
            .expect("session");
        runner
            .core
            .call("save_spec", json!({"session_id": "s-early-read", "spec": {
                "leader_id": "leader",
                "agents": [
                    {"id": "leader", "name": "leader", "role": "leader", "runtime_kind": "deepagents", "model_profile": "m"},
                    {"id": "cx", "name": "cx", "role": "worker", "runtime_kind": "codex", "model_profile": "m"},
                ],
            }}))
            .expect("spec");
        runner
            .core
            .call_in_session(
                "insert_approval",
                json!({"approval": {
                    "approval_id": "appr-early", "session_id": "s-early-read", "agent_id": "cx",
                    "run_id": "r1", "tool_call_id": "call-1", "operation_hash": "h",
                    "requested_scope": {}, "policy_revision": 1,
                }}),
            )
            .expect("approval row");
        let decide: teamagents_core::models::TeamAction = serde_json::from_value(json!({
            "action_id": "d1", "session_id": "s-early-read", "actor_id": "user",
            "kind": "approval_decision", "payload": {"approval_id": "appr-early", "decision": "once"},
        }))
        .expect("action");
        let receipt = runner.core.submit(&decide).expect("submit");
        assert!(receipt.ok, "the decision lands before the waiter registers: {:?}", receipt.error);

        assert_eq!(runner.decided_before_waiting("appr-early"), Some("once".to_string()));
        assert_eq!(runner.decided_before_waiting("ghost"), None);
    }

    /// Round 7 tightening (user-confirmed): a "session" decision answers the
    /// app-server with a single-op "accept" — never acceptForSession, whose
    /// cache is not bound to one operation_hash — and the core-bound grant
    /// auto-accepts the next identical operation without a new PENDING row,
    /// while a different operation still asks.
    #[test]
    fn session_decision_is_single_op_on_the_wire_and_cached_per_operation() {
        let runner = test_runner("s-session-grant");
        // a team spec so the core's submit validation accepts the decision
        runner.core.call("create_session", json!({"session_id": "s-session-grant", "cwd": "/tmp"})).expect("create");
        runner.core.call("set_catalog", json!({"session_id": "s-session-grant", "catalog": {
            "models": {"m": {"provider": "openai", "protocol": "openai", "model": "test"}},
            "tools": {}, "skills_paths": [], "instruction_files": [],
        }})).expect("catalog");
        runner.core.call("save_spec", json!({"session_id": "s-session-grant", "spec": {
            "leader_id": "leader",
            "agents": [{"id": "leader", "name": "leader", "role": "leader", "runtime_kind": "deepagents", "model_profile": "m"}],
            "channels": [],
        }})).expect("spec");
        // the real wire shape: discriminating fields at the top level, with
        // per-call unique threadId/turnId/itemId/startedAtMs (codex 0.154
        // schema) — the projection must ignore them or no grant ever matches
        let request = |seq: u64, command: &str| {
            json!({"method": "item/commandExecution/requestApproval",
                   "params": {"threadId": "thr-1", "turnId": format!("turn-{seq}"),
                              "itemId": format!("item-{seq}"), "startedAtMs": seq,
                              "command": command, "cwd": "/tmp"}})
        };
        let decide = |runner: &Arc<CodexRunner>, action_id: &str, approval_id: &str, decision: &str| {
            let action: teamagents_core::models::TeamAction = serde_json::from_value(json!({
                "action_id": action_id, "session_id": runner.core.session_id,
                "actor_id": "user", "kind": "approval_decision",
                "payload": {"approval_id": approval_id, "decision": decision},
            }))
            .expect("action");
            let receipt = runner.core.submit(&action).expect("decide");
            assert!(receipt.ok, "decision rejected: {receipt:?}");
        };
        let parked_ids = |runner: &Arc<CodexRunner>| {
            runner.approval_waits.lock().unwrap().keys().cloned().collect::<std::collections::HashSet<_>>()
        };
        let wait_for_park = |runner: &Arc<CodexRunner>, n: usize| {
            for _ in 0..200 {
                if parked_ids(runner).len() >= n {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("waiter never parked");
        };

        // 1. a "session" decision answers single-op accept, never acceptForSession
        let r = runner.clone();
        let waiter = std::thread::spawn(move || r.on_request("r1", request(1, "ls")));
        wait_for_park(&runner, 1);
        let approval_id = parked_ids(&runner).into_iter().next().unwrap();
        decide(&runner, "decide-1", &approval_id, "session");
        assert!(runner.resolve_approval_for(&approval_id, "session"));
        let reply = waiter.join().expect("waiter joined");
        assert_eq!(reply, json!({"decision": "accept"}), "never acceptForSession");

        // 2. the identical operation under fresh per-call ids is auto-accepted
        //    by the grant — no waiter parks (a parked waiter is what blocks on the user)
        let reply = runner.on_request("r1", request(2, "ls"));
        assert_eq!(reply, json!({"decision": "accept"}));
        assert!(runner.approval_waits.lock().unwrap().is_empty(), "the grant answered instead of parking");

        // 3. a terminal run's in-flight request declines even with a live grant
        runner.states.lock().unwrap().insert("r3".into(), TurnStatus::Failed);
        let reply = runner.on_request("r3", request(3, "ls"));
        assert_eq!(reply, json!({"decision": "decline"}), "torn-down turn must not run under the grant");

        // 4. sibling waiters parked on the same op_hash are released by the
        //    grant, their moot PENDING rows expired
        let r = runner.clone();
        let b1 = std::thread::spawn(move || r.on_request("r4", request(4, "pwd")));
        wait_for_park(&runner, 1);
        let b1_id = parked_ids(&runner).into_iter().next().unwrap();
        let r = runner.clone();
        let b2 = std::thread::spawn(move || r.on_request("r5", request(5, "pwd")));
        wait_for_park(&runner, 2);
        decide(&runner, "decide-2", &b1_id, "session");
        assert!(runner.resolve_approval_for(&b1_id, "session"));
        assert_eq!(b1.join().expect("b1"), json!({"decision": "accept"}));
        assert_eq!(b2.join().expect("b2 released by the grant"), json!({"decision": "accept"}));
        let pending = runner.core.state_brief().expect("state")
            .get("pending_approvals").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        assert!(pending.is_empty(), "the sibling's moot row is expired: {pending:?}");

        // 4b. fileChange carries no discriminating field on the wire, so a
        //     "session" decision on it never turns into a reusable grant
        let file_change = |seq: u64| {
            json!({"method": "item/fileChange/requestApproval",
                   "params": {"threadId": "thr-1", "turnId": format!("turn-{seq}"),
                              "itemId": format!("item-{seq}"), "startedAtMs": seq,
                              "reason": "write"}})
        };
        let r = runner.clone();
        let waiter = std::thread::spawn(move || r.on_request("r6", file_change(6)));
        wait_for_park(&runner, 1);
        let fc_id = parked_ids(&runner).into_iter().next().unwrap();
        decide(&runner, "decide-fc", &fc_id, "session");
        assert!(runner.resolve_approval_for(&fc_id, "session"));
        assert_eq!(waiter.join().expect("fc"), json!({"decision": "accept"}));
        let r = runner.clone();
        let waiter = std::thread::spawn(move || r.on_request("r6", file_change(7)));
        wait_for_park(&runner, 1);
        let fc_id = parked_ids(&runner).into_iter().next().unwrap();
        decide(&runner, "decide-fc-2", &fc_id, "deny");
        assert!(runner.resolve_approval_for(&fc_id, "deny"));
        assert_eq!(waiter.join().expect("fc again"), json!({"decision": "decline"}),
            "fileChange stays fail-closed: no reusable grant");

        // 5. a policy-mode change clears the grant although the core cache row
        //    survives: the same operation asks again
        runner.approvals.set_mode("full_auto");
        let r = runner.clone();
        let waiter = std::thread::spawn(move || r.on_request("r1", request(6, "ls")));
        wait_for_park(&runner, 1);
        let approval_id = parked_ids(&runner).into_iter().next().unwrap();
        decide(&runner, "decide-3", &approval_id, "deny");
        assert!(runner.resolve_approval_for(&approval_id, "deny"));
        assert_eq!(waiter.join().expect("joined"), json!({"decision": "decline"}));
    }
}
