use crate::{
    context, mcp,
    model::{self, StreamEvent},
    sandbox::SandboxMode,
    store::Store,
    tools, Result,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const FRAME: usize = 8 * 1024 * 1024;
const SOCKET: &str = "swarm.sock";
const PROTOCOL: u32 = 3;
const QUOTA_PAUSE: Duration = Duration::from_secs(30);
const INCOMPATIBLE: &str = "incompatible session protocol; stop the old daemon with its matching client, then restart";

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

struct Location {
    session: PathBuf,
    workspace: PathBuf,
    dir: File,
}

impl Location {
    fn new(session: &Path, workspace: &Path) -> Result<Self> {
        let workspace = workspace.canonicalize().map_err(err)?;
        if !workspace.is_dir() {
            return Err("workspace is not a directory".into());
        }
        crate::oauth::ensure_disjoint(&workspace)?;
        crate::oauth::ensure_disjoint(session)?;
        DirBuilder::new().recursive(true).mode(0o700).create(session).map_err(err)?;
        let session = session.canonicalize().map_err(err)?;
        if session.starts_with(&workspace) || workspace.starts_with(&session) {
            return Err("session storage and workspace must be separate, non-overlapping directories".into());
        }
        let dir = File::open(&session).map_err(err)?;
        let meta = dir.metadata().map_err(err)?;
        // The socket is a user control interface, not an agent capability.
        if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o022 != 0 {
            return Err("session directory must be owned by you and not writable by group or others".into());
        }
        Ok(Self { session, workspace, dir })
    }

    fn socket(&self) -> PathBuf {
        // On Linux, address the same socket inode even when the full XDG path exceeds sun_path.
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            PathBuf::from(format!("/proc/self/fd/{}/{SOCKET}", self.dir.as_raw_fd()))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = &self.dir;
            self.session.join(SOCKET)
        }
    }
}

fn read_frame(stream: &UnixStream) -> Result<Value> {
    let mut buf = Vec::new();
    BufReader::new(stream).take((FRAME + 1) as u64).read_until(b'\n', &mut buf).map_err(err)?;
    if buf.len() > FRAME || buf.last() != Some(&b'\n') {
        return Err("incomplete or oversized daemon frame".into());
    }
    serde_json::from_slice(&buf).map_err(err)
}

fn write_frame(mut stream: &UnixStream, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(value).map_err(err)?;
    if bytes.len() >= FRAME {
        return Err("daemon frame exceeds 8 MiB".into());
    }
    bytes.push(b'\n');
    stream.write_all(&bytes).map_err(err)
}

fn timeouts(stream: &UnixStream, seconds: u64) -> Result<()> {
    let timeout = Some(Duration::from_secs(seconds));
    stream.set_read_timeout(timeout).map_err(err)?;
    stream.set_write_timeout(timeout).map_err(err)
}

pub fn request_with_caps(
    session: &Path,
    workspace: &Path,
    body: &Value,
    acknowledged_provider: Option<&str>,
    provider_switching: Option<&AtomicBool>,
    codex_v1: Option<&AtomicBool>,
) -> Result<Value> {
    let loc = Location::new(session, workspace)?;
    let socket = loc.socket();
    let stream = match UnixStream::connect(&socket) {
        Ok(stream) => stream,
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => {
            if body["op"] == "stop" {
                return Err("no session daemon is listening".into());
            }
            let mut child = Command::new(std::env::current_exe().map_err(err)?)
                .arg("daemon")
                .arg("--session")
                .arg(&loc.session)
                .arg("--workspace")
                .arg(&loc.workspace)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .process_group(0)
                .spawn()
                .map_err(err)?;
            let deadline = Instant::now() + Duration::from_secs(6);
            let mut exited = false;
            let mut diagnostic = String::new();
            loop {
                if let Ok(stream) = UnixStream::connect(&socket) {
                    // A long-lived terminal may start/stop several sessions. Reap our own child.
                    thread::spawn(move || {
                        let _ = child.wait();
                    });
                    break stream;
                }
                if !exited && child.try_wait().map_err(err)?.is_some() {
                    exited = true;
                    if let Some(stderr) = child.stderr.take() {
                        let _ = stderr.take(8192).read_to_string(&mut diagnostic);
                    }
                    if !diagnostic.contains("session is already served") {
                        return Err(format!("daemon exited during startup: {}", diagnostic.trim()));
                    }
                }
                if Instant::now() >= deadline {
                    return Err(format!("daemon did not become ready: {}", diagnostic.trim()));
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
        Err(e) => return Err(err(e)),
    };
    timeouts(&stream, 10)?;
    let workspace = loc.workspace.as_os_str().as_encoded_bytes();
    let frame = |b: &Value| json!({"protocol":PROTOCOL,"workspace":workspace,"body":b});
    // An old daemon ignores unrecognized fields. Negotiate using a read-only request
    // on the same connection before ever sending a mutating body.
    let mut hello = frame(&json!({"op":"hello"}));
    let op = body["op"].as_str();
    let needs_provider = matches!(
        op,
        Some(
            "submit"
                | "continue"
                | "model"
                | "providers"
                | "model_list"
                | "steer"
                | "compact"
                | "diff"
                | "skills"
                | "mcp"
                | "files"
        )
    );
    if needs_provider {
        hello["query_provider"] = json!(true);
    }
    write_frame(&stream, &hello)?;
    let greeting = read_frame(&stream)?;
    if let Some(error) = greeting["error"].as_str() {
        return Err(error.to_owned());
    }
    if greeting["value"]["protocol"] != PROTOCOL {
        return Err(INCOMPATIBLE.into());
    }
    if needs_provider {
        if let Some(flag) = provider_switching {
            flag.store(greeting["value"]["provider_switching"] == true, Ordering::Relaxed);
        }
        if let Some(flag) = codex_v1 {
            flag.store(greeting["value"]["codex_v1"] == true, Ordering::Relaxed);
        }
    }
    if op == Some("model") && greeting["value"]["model_settings"] != true {
        return Err(
            "this session daemon predates /model; let active work finish, then run teamagents stop and reopen chat"
                .into(),
        );
    }
    if (matches!(op, Some("providers" | "model_list")) || (op == Some("model") && body.get("provider").is_some()))
        && greeting["value"]["provider_switching"] != true
    {
        return Err(
            "this session daemon predates live /model provider switching; let active work finish, then run teamagents stop and reopen chat"
                .into(),
        );
    }
    let needs_codex = matches!(op, Some("steer" | "compact" | "diff" | "skills" | "mcp" | "files"))
        || (matches!(op, Some("submit" | "continue")) && body["sandbox"].as_str() == Some("read_only"));
    if needs_codex && greeting["value"]["codex_v1"] != true {
        return Err(
            "this session daemon predates Codex parity features; let active work finish, then run teamagents stop and restart"
                .into(),
        );
    }
    if matches!(op, Some("submit" | "continue")) {
        // Protocol 3 daemons predating subscription support always use API billing.
        let actual = greeting["value"]["provider"].as_str().unwrap_or("api");
        let overridden = greeting["value"]["provider_overridden"] == true;
        if acknowledged_provider != Some(actual) {
            if let Some(explicit) = model::explicit_env_provider()? {
                if actual != explicit {
                    return Err(format!(
                        "Session uses {actual}, but this client selected {explicit}. No task was submitted. Switch with /model provider {explicit}, or run teamagents stop in this workspace and restart with the intended provider."
                    ));
                }
            } else if !overridden {
                let desired = model::default_provider_name()?;
                if actual != desired {
                    return Err(format!(
                        "Session uses {actual}, but this client selected {desired}. No task was submitted. Switch with /model provider {desired}, or run teamagents stop in this workspace and restart with the intended provider."
                    ));
                }
            }
        }
    }
    write_frame(&stream, &frame(body))?;
    // A broken connection after writing is ambiguous; never resubmit a mutation.
    let reply =
        read_frame(&stream).map_err(|e| format!("daemon reply unavailable ({e}); inspect tasks before retrying"))?;
    if let Some(error) = reply["error"].as_str() {
        return Err(error.to_owned());
    }
    reply.get("value").cloned().ok_or_else(|| "invalid daemon response".into())
}

struct SocketGuard(PathBuf);
impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[derive(Clone, Default)]
struct LiveState {
    phase: String,
    reasoning: String,
    commentary: String,
    active_tools: Vec<String>,
    shell_tail: String,
}

impl LiveState {
    fn to_json(&self) -> Value {
        json!({
            "phase": self.phase,
            "reasoning": self.reasoning,
            "commentary": self.commentary,
            "active_tools": self.active_tools,
            "shell_tail": self.shell_tail,
        })
    }
}

type LiveMap = Arc<Mutex<HashMap<String, LiveState>>>;

fn append_bounded_tail(buf: &mut String, delta: &str, max_bytes: usize) {
    buf.push_str(delta);
    if buf.len() > max_bytes {
        let mut drop_from = buf.len() - max_bytes;
        while drop_from < buf.len() && !buf.is_char_boundary(drop_from) {
            drop_from += 1;
        }
        buf.drain(..drop_from);
    }
}

enum Completion {
    Model(String, usize, std::result::Result<model::ModelOutput, model::ModelError>),
    ToolOp { op_id: String, task: String, op: String, result: Result<String> },
    Compacted { task: String, head_keep: usize, tail_keep: usize, summary: String },
    ManualCompacted { task: String, head_keep: usize, tail_keep: usize, summary: String },
}

#[derive(Clone)]
struct Notifier {
    tx: Sender<Completion>,
    wake: Arc<UnixStream>,
}

impl Notifier {
    fn send(&self, item: Completion) {
        let _ = self.tx.send(item);
        // A full wakeup socket already has POLLIN queued for the event loop.
        let _ = (&*self.wake).write(&[1]);
    }
}

fn drain_wake(wake_rx: &mut UnixStream) {
    let mut buf = [0u8; 64];
    loop {
        match wake_rx.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => continue,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

fn wait_for_activity(listener: &UnixListener, wake_rx: &UnixStream, timeout_ms: i32) -> Result<()> {
    let mut fds = [
        libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        libc::pollfd { fd: wake_rx.as_raw_fd(), events: libc::POLLIN, revents: 0 },
    ];
    loop {
        // SAFETY: `fds` points to 2 valid `pollfd` entries backed by open descriptors for the call duration.
        let rc = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) };
        if rc >= 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(err(e));
        }
    }
}

fn launch(
    store: &mut Store,
    workspace: &Path,
    notifier: &Notifier,
    live: &LiveMap,
    tool_first: bool,
    quota_pause: &mut Option<(Instant, String)>,
) -> Result<bool> {
    for tool in [tool_first, !tool_first] {
        if tool {
            if let Some(mut job) = store.claim_tool_job()? {
                // Only the store can produce verify, from the user's original --check.
                if job.action["op"] == "verify" {
                    job.action["op"] = json!("shell");
                }
                let workspace = workspace.to_path_buf();
                let notifier = notifier.clone();
                let op_id = job.op_id;
                let task = job.task;
                let action = job.action;
                let sandbox = job.sandbox;
                let op_name = action["op"].as_str().unwrap_or("tool").to_owned();
                if matches!(op_name.as_str(), "wb_read" | "wb_write") {
                    let result = store.execute_whiteboard_op(&task, &action);
                    notifier.send(Completion::ToolOp { op_id, task, op: op_name, result });
                    return Ok(true);
                }
                if let Ok(mut guard) = live.lock() {
                    let st = guard.entry(task.clone()).or_default();
                    st.phase = "tool".into();
                    st.active_tools.push(op_name.clone());
                    if op_name == "shell" {
                        st.shell_tail.clear();
                    }
                }
                let live_cb = Arc::clone(live);
                let task_cb = task.clone();
                thread::spawn(move || {
                    let chunk_fn = |chunk: &[u8]| {
                        let s = String::from_utf8_lossy(chunk);
                        if let Ok(mut guard) = live_cb.lock() {
                            if let Some(st) = guard.get_mut(&task_cb) {
                                append_bounded_tail(&mut st.shell_tail, &s, 4096);
                            }
                        }
                    };
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        tools::execute_with_options(&workspace, &action, sandbox, Some(&chunk_fn))
                    }))
                    .unwrap_or_else(|_| Err("tool worker panicked".into()));
                    notifier.send(Completion::ToolOp { op_id, task, op: op_name, result });
                });
                return Ok(true);
            }
        } else if let Some(turn) = store.claim()? {
            if let Some((until, reason)) = quota_pause.as_ref() {
                if Instant::now() < *until {
                    notifier.send(Completion::Model(
                        turn.task,
                        0,
                        Err(model::ModelError::QuotaExceeded(reason.clone())),
                    ));
                    return Ok(true);
                }
                *quota_pause = None;
            }
            if let Ok(mut guard) = live.lock() {
                guard.insert(
                    turn.task.clone(),
                    LiveState {
                        phase: "thinking".into(),
                        reasoning: String::new(),
                        commentary: String::new(),
                        active_tools: Vec::new(),
                        shell_tail: String::new(),
                    },
                );
            }
            let notifier = notifier.clone();
            let live_cb = Arc::clone(live);
            thread::spawn(move || {
                let history_bytes = model::history_bytes(&turn.messages);
                let task_cb = turn.task.clone();
                let stream_fn = move |ev: StreamEvent| {
                    if let Ok(mut guard) = live_cb.lock() {
                        let st = guard.entry(task_cb.clone()).or_default();
                        match ev {
                            StreamEvent::ReasoningDelta(d) => append_bounded_tail(&mut st.reasoning, &d, 4096),
                            StreamEvent::CommentaryDelta(d) => append_bounded_tail(&mut st.commentary, &d, 4096),
                            StreamEvent::ToolCallStarted(name) => {
                                st.phase = "tool".into();
                                if !st.active_tools.contains(&name) {
                                    st.active_tools.push(name);
                                }
                            }
                        }
                    }
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    model::complete_streaming(&turn, Some(&stream_fn))
                }))
                .unwrap_or_else(|_| Err(model::ModelError::Fatal("model worker panicked".into())));
                notifier.send(Completion::Model(turn.task, history_bytes, result));
            });
            return Ok(true);
        }
    }
    Ok(false)
}

fn settle(
    store: &mut Store,
    done: Completion,
    notifier: &Notifier,
    live: &LiveMap,
    active: &mut usize,
    quota_pause: &mut Option<(Instant, String)>,
) -> Result<()> {
    match &done {
        Completion::Model(id, _, _) => {
            if let Ok(mut guard) = live.lock() {
                guard.remove(id);
            }
            if store.discard_late_completion(id)? {
                return Ok(());
            }
        }
        Completion::ToolOp { op_id, task, op, .. } => {
            if let Ok(mut guard) = live.lock() {
                if let Some(st) = guard.get_mut(task) {
                    if let Some(pos) = st.active_tools.iter().position(|x| x == op) {
                        st.active_tools.remove(pos);
                    }
                    if st.active_tools.is_empty() {
                        guard.remove(task);
                    }
                }
            }
            if store.discard_late_tool_op(op_id)? {
                return Ok(());
            }
        }
        Completion::Compacted { task, .. } => {
            if store.discard_late_completion(task)? {
                return Ok(());
            }
        }
        Completion::ManualCompacted { .. } => {}
    }
    match done {
        Completion::ToolOp { op_id, result, .. } => {
            store.tool_result_op(&op_id, result.as_deref().map_err(String::as_str))
        }
        Completion::Model(id, _, Err(model::ModelError::Fatal(e))) => store.fail(&id, &e),
        Completion::Model(id, _, Err(model::ModelError::QuotaExceeded(e))) => {
            *quota_pause = Some((Instant::now() + QUOTA_PAUSE, e.clone()));
            store.fail_quota(&id, &e)
        }
        Completion::Model(id, _, Err(model::ModelError::InvalidAction(e))) => store.reject_action(&id, &e),
        Completion::Model(id, _, Err(model::ModelError::NeedsCompaction(reason))) => {
            if let Some(job) = store.prepare_compaction(&id)? {
                let notifier = notifier.clone();
                *active += 1;
                thread::spawn(move || {
                    let summary = std::panic::catch_unwind(|| {
                        model::compact_history(&job.agent, &job.task, &job.middle, job.settings.as_ref())
                    })
                    .unwrap_or_else(|_| model::fallback_compact(&job.middle));
                    notifier.send(Completion::Compacted {
                        task: job.task,
                        head_keep: job.head_keep,
                        tail_keep: job.tail_keep,
                        summary,
                    });
                });
                Ok(())
            } else {
                store.fail(&id, &reason)
            }
        }
        Completion::Compacted { task, head_keep, tail_keep, summary } => {
            store.apply_compaction(&task, head_keep, tail_keep, &summary)
        }
        Completion::ManualCompacted { task, head_keep, tail_keep, summary } => {
            store.apply_manual_compaction(&task, head_keep, tail_keep, &summary)
        }
        Completion::Model(id, history_bytes, Ok(output)) => {
            if let Some(usage) = &output.usage {
                store.record_usage(&id, usage, history_bytes)?;
            }
            if let Err(e) = store.apply_model_output(&id, &output) {
                store.reject_action(&id, &e)?;
            }
            Ok(())
        }
    }
}

fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key].as_str().filter(|s| !s.trim().is_empty()).ok_or_else(|| format!("expected nonempty {key}"))
}

pub(crate) const EFFORTS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max"];

fn effective_provider_raw(store: &Store) -> Result<String> {
    if let Some(name) = store.active_provider()? {
        return Ok(name);
    }
    model::default_provider_name()
}

fn effective_provider(store: &Store) -> Result<String> {
    if let Some(name) = store.active_provider()? {
        model::resolve_provider(&name)?;
        return Ok(name);
    }
    model::default_provider_name()
}

fn configure_model(store: &mut Store, body: &Value) -> Result<Value> {
    if let Some(value) = body.get("provider") {
        if value.is_null() {
            store.set_active_provider(None)?;
        } else {
            let name = value.as_str().filter(|s| !s.is_empty()).ok_or("invalid provider name")?;
            model::resolve_provider(name)?;
            store.set_active_provider(Some(name))?;
        }
    }
    let provider = effective_provider(store)?;
    let mut settings = store.model_settings(&provider, None)?;
    if let Some(value) = body.get("model") {
        settings.model = if value.is_null() {
            None
        } else {
            Some(value.as_str().filter(|s| model::valid_model_id(s)).ok_or("invalid model ID")?.to_owned())
        };
    }
    if let Some(value) = body.get("effort") {
        settings.effort = if value.is_null() {
            None
        } else {
            let effort = value
                .as_str()
                .filter(|s| EFFORTS.contains(s))
                .ok_or_else(|| format!("reasoning effort must be default or one of {}", EFFORTS.join(", ")))?;
            Some(effort.to_owned())
        };
    }
    if body.get("model").is_some() || body.get("effort").is_some() {
        store.set_model_settings(&provider, &settings)?;
    }
    Ok(json!({
        "provider": provider,
        "model": settings.model.or_else(|| model::default_model_for(&provider)),
        "effort": settings.effort,
        "overridden": store.active_provider()?.is_some(),
    }))
}

#[allow(clippy::too_many_arguments)]
fn dispatch(
    store: &mut Store,
    workspace_path: &Path,
    workspace: &Value,
    frame: &Value,
    live: &LiveMap,
    notifier: &Notifier,
    active: &mut usize,
) -> Result<(Value, bool)> {
    if frame["protocol"] != PROTOCOL {
        return Err(INCOMPATIBLE.into());
    }
    if &frame["workspace"] != workspace {
        return Err("session belongs to another workspace".into());
    }
    let b = &frame["body"];
    let parse_sandbox = |b: &Value| -> Result<SandboxMode> {
        match b.get("sandbox").and_then(Value::as_str) {
            Some(s) => SandboxMode::parse(s),
            None => Ok(if b.get("staged").and_then(Value::as_bool).unwrap_or(false) {
                SandboxMode::Staged
            } else {
                SandboxMode::Direct
            }),
        }
    };
    let value = match field(b, "op")? {
        "submit" => {
            let agent = field(b, "agent")?;
            let prompt = field(b, "text")?;
            let check = b.get("check").filter(|v| !v.is_null()).map(|_| field(b, "check")).transpose()?;
            let review = b["review"].as_bool().ok_or("expected boolean review policy")?;
            let sandbox = parse_sandbox(b)?;
            let provider = effective_provider(store)?;
            let settings = store.model_settings(&provider, model::default_model_for(&provider).as_deref())?;
            let mcp_names = mcp::configured_server_names();
            let ctx = context::build_root_context_prefix(workspace_path, sandbox, &mcp_names);
            let id =
                store.submit_with_full_options(agent, prompt, check, review, sandbox, &settings, ctx.as_deref())?;
            json!({"id":id})
        }
        "continue" => {
            let id = field(b, "id")?;
            let prompt = field(b, "text")?;
            let check = b.get("check").filter(|v| !v.is_null()).map(|_| field(b, "check")).transpose()?;
            let review = b["review"].as_bool().ok_or("expected boolean review policy")?;
            let sandbox = parse_sandbox(b)?;
            let provider = effective_provider(store)?;
            let settings = store.model_settings(&provider, model::default_model_for(&provider).as_deref())?;
            let (root, node) =
                store.continue_tree_with_full_options(id, prompt, check, review, sandbox, &settings, None)?;
            json!({"id":root, "node":node})
        }
        "steer" => store.steer(field(b, "id")?, field(b, "text")?)?,
        "compact" => {
            let job = store.prepare_manual_compaction(field(b, "id")?)?;
            let task_id = job.task.clone();
            let count = job.middle.len();
            let notifier = notifier.clone();
            *active += 1;
            thread::spawn(move || {
                let summary = std::panic::catch_unwind(|| {
                    model::compact_history(&job.agent, &job.task, &job.middle, job.settings.as_ref())
                })
                .unwrap_or_else(|_| model::fallback_compact(&job.middle));
                notifier.send(Completion::ManualCompacted {
                    task: job.task,
                    head_keep: job.head_keep,
                    tail_keep: job.tail_keep,
                    summary,
                });
            });
            json!({"ok": true, "task": task_id, "messages": count})
        }
        "diff" => {
            let base = b.get("base").and_then(Value::as_str);
            let commit = b.get("commit").and_then(Value::as_str);
            let text = context::git_diff_summary(workspace_path, base, commit)?;
            json!({"diff": text})
        }
        "skills" => json!({"skills": context::discover_skills(workspace_path)}),
        "mcp" => json!({"config_path": mcp::mcp_config_path_display(), "servers": mcp::list_servers()?}),
        "files" => {
            let limit = b.get("limit").and_then(Value::as_u64).unwrap_or(1000).min(2000) as usize;
            json!({"files": context::workspace_file_index(workspace_path, limit)})
        }
        "whiteboard" => store.whiteboard(field(b, "id")?)?,
        "wb_write" => {
            let id = field(b, "id")?;
            let key = field(b, "key")?;
            let mode = b.get("mode").and_then(Value::as_str).unwrap_or("set");
            let kind = b.get("kind").filter(|v| !v.is_null()).map(|_| field(b, "kind")).transpose()?;
            let content = b.get("content").filter(|v| !v.is_null()).map(|_| field(b, "content")).transpose()?;
            store.user_wb_write(id, key, mode, kind, content)?
        }
        "model" => configure_model(store, b)?,
        "providers" => {
            let active = effective_provider_raw(store)?;
            let providers = model::list_providers()?;
            json!({
                "active": active,
                "overridden": store.active_provider()?.is_some(),
                "config_path": model::providers_config_path_display(),
                "providers": providers,
            })
        }
        "model_list" => {
            let provider = match b.get("provider").filter(|v| !v.is_null()) {
                Some(v) => v.as_str().filter(|s| !s.is_empty()).ok_or("invalid provider name")?.to_owned(),
                None => effective_provider(store)?,
            };
            let models = model::list_provider_models(&provider)?;
            json!({"provider": provider, "models": models})
        }
        "members" => store.agents()?,
        "tasks" => store.tasks()?,
        "status" => {
            let id = field(b, "id")?;
            let mut task = store.task(id)?;
            if let Ok(guard) = live.lock() {
                let target_id = task["active_root"].as_str().unwrap_or(id);
                if let Some(st) = guard.get(target_id).or_else(|| guard.get(id)) {
                    task["live"] = st.to_json();
                }
                if let Some(nodes) = task["tree_nodes"].as_array() {
                    let mut children = serde_json::Map::new();
                    for node in nodes {
                        if let Some(tid) = node["task"].as_str() {
                            if tid != id {
                                if let Some(st) = guard.get(tid) {
                                    children.insert(tid.to_owned(), st.to_json());
                                }
                            }
                        }
                    }
                    if !children.is_empty() {
                        task["live_children"] = Value::Object(children);
                    }
                }
            }
            task
        }
        "answer" => {
            store.answer(field(b, "id")?, field(b, "text")?)?;
            json!({"ok":true})
        }
        "cancel" => {
            store.cancel(field(b, "id")?)?;
            json!({"ok":true})
        }
        "delete" => {
            let root = store.delete(field(b, "id")?)?;
            json!({"ok":true, "root":root})
        }
        "approve" | "deny" => {
            store.approve(field(b, "id")?, b["op"] == "approve")?;
            json!({"ok":true})
        }
        "stop" if *active == 0 => return Ok((json!({"ok":true}), true)),
        "stop" => return Err("work is in flight; retry stop after the current operation".into()),
        _ => return Err("unknown user operation".into()),
    };
    Ok((value, false))
}

pub fn serve(session: &Path, workspace: &Path) -> Result<()> {
    // Set before starting any threads. SQLite sidecars and the control socket are private.
    unsafe {
        libc::umask(0o077);
    }
    let loc = Location::new(session, workspace)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(loc.session.join("swarm.lock"))
        .map_err(err)?;
    lock.try_lock().map_err(|e| format!("session is already served or cannot be locked: {e}"))?;
    let db = loc.session.join("swarm.sqlite3");
    for file in [
        &db,
        &db.with_extension("sqlite3-wal"),
        &db.with_extension("sqlite3-shm"),
        &db.with_extension("sqlite3-journal"),
    ] {
        match fs::symlink_metadata(file) {
            Ok(m)
                if !m.is_file() || m.nlink() != 1 || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 =>
            {
                return Err(format!("refusing non-private or linked state file: {}", file.display()))
            }
            Ok(_) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(err(e)),
        }
    }
    let mut store = Store::open(&db)?;
    store.bind_workspace(&loc.workspace)?;
    store.recover()?;
    let socket = loc.socket();
    match fs::symlink_metadata(&socket) {
        Ok(m) if m.file_type().is_socket() => fs::remove_file(&socket).map_err(err)?,
        Ok(_) => return Err("refusing to replace a non-socket session entry".into()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => return Err(err(e)),
    }
    let listener = UnixListener::bind(&socket).map_err(err)?;
    let _socket_guard = SocketGuard(socket);
    listener.set_nonblocking(true).map_err(err)?;
    let (mut wake_rx, wake_tx) = UnixStream::pair().map_err(err)?;
    wake_rx.set_nonblocking(true).map_err(err)?;
    wake_tx.set_nonblocking(true).map_err(err)?;
    let workspace_identity = json!(loc.workspace.as_os_str().as_encoded_bytes());
    let (tx, rx) = mpsc::channel();
    let notifier = Notifier { tx, wake: Arc::new(wake_tx) };
    let live: LiveMap = Arc::new(Mutex::new(HashMap::new()));
    let mut active = 0;
    let mut tool_first = true;
    let mut wake = true; // Also check persisted queues immediately after recovery.
    let mut quota_pause: Option<(Instant, String)> = None;
    loop {
        drain_wake(&mut wake_rx);
        for done in rx.try_iter() {
            active -= 1;
            settle(&mut store, done, &notifier, &live, &mut active, &mut quota_pause)?;
            wake = true;
        }
        // Bound each batch so a busy client cannot starve the agents.
        let mut batch_full = true;
        for _ in 0..16 {
            let stream = match listener.accept() {
                Ok((s, _)) => s,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    batch_full = false;
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(err(e)),
            };
            timeouts(&stream, 2)?;
            let result: Result<(Value, bool)> = (|| {
                let hello = read_frame(&stream)?;
                if hello["protocol"] != PROTOCOL || hello["body"]["op"] != "hello" {
                    return Err(INCOMPATIBLE.into());
                }
                let mut welcome = json!({"protocol":PROTOCOL});
                if hello["query_provider"] == true {
                    welcome["provider"] = json!(effective_provider_raw(&store)?);
                    welcome["model_settings"] = json!(true);
                    welcome["provider_switching"] = json!(true);
                    welcome["codex_v1"] = json!(true);
                    welcome["provider_overridden"] = json!(store.active_provider()?.is_some());
                }
                write_frame(&stream, &json!({"value":welcome}))?;
                let frame = read_frame(&stream)?;
                let result =
                    dispatch(&mut store, &loc.workspace, &workspace_identity, &frame, &live, &notifier, &mut active)?;
                if matches!(frame["body"]["op"].as_str(), Some("submit" | "continue")) {
                    quota_pause = None;
                }
                wake |= matches!(
                    frame["body"]["op"].as_str(),
                    Some("submit" | "continue" | "steer" | "answer" | "approve" | "deny" | "cancel" | "wb_write")
                );
                Ok(result)
            })();
            let (reply, stop) = match result {
                Ok((v, stop)) => (json!({"value":v}), stop),
                Err(e) => (json!({"error":e}), false),
            };
            if stop {
                drop(_socket_guard);
                drop(listener);
                drop(store);
                drop(lock);
                let _ = write_frame(&stream, &reply);
                return Ok(());
            }
            if let Err(e) = write_frame(&stream, &reply) {
                if e == "daemon frame exceeds 8 MiB" {
                    let _ = write_frame(&stream, &json!({"error":e}));
                }
            }
        }
        while wake {
            if !launch(&mut store, &loc.workspace, &notifier, &live, tool_first, &mut quota_pause)? {
                wake = false;
                break;
            }
            active += 1;
            tool_first = !tool_first;
        }
        if !batch_full && !wake {
            wait_for_activity(&listener, &wake_rx, 1000)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifier_wakes_blocked_poll_without_sleep_latency() {
        let dir = std::env::temp_dir().join(format!("ta-wake-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let sock_path = dir.join("test.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let (mut wake_rx, wake_tx) = UnixStream::pair().unwrap();
        wake_rx.set_nonblocking(true).unwrap();
        wake_tx.set_nonblocking(true).unwrap();
        let (tx, rx) = mpsc::channel();
        let notifier = Notifier { tx, wake: Arc::new(wake_tx) };
        let worker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(5));
            notifier.send(Completion::ToolOp {
                op_id: "op1".into(),
                task: "t1".into(),
                op: "shell".into(),
                result: Ok("done".into()),
            });
        });
        let start = Instant::now();
        wait_for_activity(&listener, &wake_rx, 1000).unwrap();
        let elapsed = start.elapsed();
        drain_wake(&mut wake_rx);
        assert!(matches!(
            rx.try_recv().unwrap(),
            Completion::ToolOp { task, result: Ok(msg), .. } if task == "t1" && msg == "done"
        ));
        assert!(elapsed < Duration::from_millis(100), "poll took {elapsed:?}");
        worker.join().unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
