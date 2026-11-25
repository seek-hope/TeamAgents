use crate::{model, store::Store, tools, Result};
use serde_json::{json, Value};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

const FRAME: usize = 4 * 1024 * 1024;
const SOCKET: &str = "swarm.sock";
const PROTOCOL: u32 = 3;
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
        return Err("daemon frame exceeds 4 MiB".into());
    }
    bytes.push(b'\n');
    stream.write_all(&bytes).map_err(err)
}

fn timeouts(stream: &UnixStream, seconds: u64) -> Result<()> {
    let timeout = Some(Duration::from_secs(seconds));
    stream.set_read_timeout(timeout).map_err(err)?;
    stream.set_write_timeout(timeout).map_err(err)
}

pub fn request(session: &Path, workspace: &Path, body: &Value) -> Result<Value> {
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
    let desired = if matches!(body["op"].as_str(), Some("submit" | "model")) {
        hello["query_provider"] = json!(true);
        Some(model::provider_name()?)
    } else {
        None
    };
    write_frame(&stream, &hello)?;
    let greeting = read_frame(&stream)?;
    if let Some(error) = greeting["error"].as_str() {
        return Err(error.to_owned());
    }
    if greeting["value"]["protocol"] != PROTOCOL {
        return Err(INCOMPATIBLE.into());
    }
    if body["op"] == "model" && greeting["value"]["model_settings"] != true {
        return Err(
            "this session daemon predates /model; let active work finish, then run teamagents stop and reopen chat"
                .into(),
        );
    }
    if let Some(desired) = desired {
        // Protocol 3 daemons predating subscription support always use API billing.
        let actual = greeting["value"]["provider"].as_str().unwrap_or("api");
        if actual != desired {
            let detail =
                if body["op"] == "submit" { "No task was submitted." } else { "Model settings were not accessed." };
            return Err(format!("Session uses {actual}, but this client selected {desired}. {detail} Run teamagents stop in this workspace and restart with the intended provider."));
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

fn limit(name: &str, default: u32, max: u32) -> Result<u32> {
    match std::env::var(name) {
        Err(std::env::VarError::NotPresent) => Ok(default),
        Ok(s) if s.parse::<u32>().is_ok_and(|n| n > 0 && n <= max) => Ok(s.parse().expect("validated integer")),
        _ => Err(format!("{name} must be an integer from 1 to {max}")),
    }
}

enum Completion {
    Model(String, Result<Value>),
    Tool(String, Result<String>),
}

fn launch(store: &mut Store, workspace: &Path, tx: &Sender<Completion>, tool_first: bool) -> Result<bool> {
    for tool in [tool_first, !tool_first] {
        if tool {
            if let Some((id, mut action)) = store.claim_tool()? {
                // Only the store can produce verify, from the user's original --check.
                if action["op"] == "verify" {
                    action["op"] = json!("shell");
                }
                let workspace = workspace.to_path_buf();
                let tx = tx.clone();
                thread::spawn(move || {
                    let result = std::panic::catch_unwind(|| tools::execute(&workspace, &action))
                        .unwrap_or_else(|_| Err("tool worker panicked".into()));
                    let _ = tx.send(Completion::Tool(id, result));
                });
                return Ok(true);
            }
        } else if let Some(turn) = store.claim()? {
            let tx = tx.clone();
            thread::spawn(move || {
                let result = std::panic::catch_unwind(|| {
                    model::complete(&turn.agent, &turn.task, &turn.messages, &turn.kind, turn.settings.as_ref())
                })
                .unwrap_or_else(|_| Err("model worker panicked".into()));
                let _ = tx.send(Completion::Model(turn.task, result));
            });
            return Ok(true);
        }
    }
    Ok(false)
}

fn settle(store: &mut Store, done: Completion) -> Result<()> {
    let id = match &done {
        Completion::Model(id, _) | Completion::Tool(id, _) => id,
    };
    // A user cancellation can win while I/O is in flight. A late model response grants no authority.
    if store.discard_late_completion(id)? {
        return Ok(());
    }
    match done {
        Completion::Tool(id, result) => store.tool_result(&id, result.as_deref().map_err(String::as_str)),
        Completion::Model(id, Err(e)) => store.fail(&id, &e),
        Completion::Model(id, Ok(action)) => {
            let result = match action["op"].as_str() {
                Some("read_file" | "edit_file" | "shell") => store.request_tool(&id, &action),
                Some("ask" | "handoff" | "finish" | "members" | "spawn" | "delegate" | "send") => {
                    store.apply(&id, &action)
                }
                _ => Err("unrecognized model action".into()),
            };
            if let Err(e) = result {
                store.fail(&id, &e)?;
            }
            Ok(())
        }
    }
}

fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key].as_str().filter(|s| !s.trim().is_empty()).ok_or_else(|| format!("expected nonempty {key}"))
}

pub(crate) const EFFORTS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max"];

fn configure_model(store: &mut Store, body: &Value) -> Result<Value> {
    let provider = model::provider_name()?;
    let mut settings = store.model_settings(provider, None)?;
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
        store.set_model_settings(provider, &settings)?;
    }
    Ok(json!({"provider":provider, "model":settings.model.or_else(model::default_model), "effort":settings.effort}))
}

fn dispatch(store: &mut Store, workspace: &Value, frame: &Value, active: usize) -> Result<(Value, bool)> {
    if frame["protocol"] != PROTOCOL {
        return Err(INCOMPATIBLE.into());
    }
    if &frame["workspace"] != workspace {
        return Err("session belongs to another workspace".into());
    }
    let b = &frame["body"];
    let value = match field(b, "op")? {
        "submit" => {
            let agent = field(b, "agent")?;
            let prompt = field(b, "text")?;
            let check = b.get("check").filter(|v| !v.is_null()).map(|_| field(b, "check")).transpose()?;
            let review = b["review"].as_bool().ok_or("expected boolean review policy")?;
            let settings = store.model_settings(model::provider_name()?, model::default_model().as_deref())?;
            let id = store.submit_with_model(agent, prompt, check, review, &settings)?;
            json!({"id":id})
        }
        "model" => configure_model(store, b)?,
        "members" => store.agents()?,
        "tasks" => store.tasks()?,
        "status" => store.task(field(b, "id")?)?,
        "answer" => {
            store.answer(field(b, "id")?, field(b, "text")?)?;
            json!({"ok":true})
        }
        "cancel" => {
            store.cancel(field(b, "id")?)?;
            json!({"ok":true})
        }
        "approve" | "deny" => {
            store.approve(field(b, "id")?, b["op"] == "approve")?;
            json!({"ok":true})
        }
        "stop" if active == 0 => return Ok((json!({"ok":true}), true)),
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
    let max_agents = limit("TEAMAGENTS_MAX_AGENTS", 8, 1024)?;
    let budget = limit("TEAMAGENTS_TURN_BUDGET", 32, 10_000)?;
    let concurrency = limit("TEAMAGENTS_CONCURRENCY", 4, 64)? as usize;
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
    let mut store = Store::open(&db, max_agents, budget)?;
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
    let workspace_identity = json!(loc.workspace.as_os_str().as_encoded_bytes());
    let (tx, rx) = mpsc::channel();
    let mut active = 0;
    let mut tool_first = true;
    let mut wake = true; // Also check persisted queues immediately after recovery.
    loop {
        for done in rx.try_iter() {
            active -= 1;
            settle(&mut store, done)?;
            wake = true;
        }
        // Bound each batch so a busy client cannot starve the agents.
        for _ in 0..16 {
            let stream = match listener.accept() {
                Ok((s, _)) => s,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
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
                    welcome["provider"] = json!(model::provider_name()?);
                    welcome["model_settings"] = json!(true);
                }
                write_frame(&stream, &json!({"value":welcome}))?;
                let frame = read_frame(&stream)?;
                let result = dispatch(&mut store, &workspace_identity, &frame, active)?;
                wake |=
                    matches!(frame["body"]["op"].as_str(), Some("submit" | "answer" | "approve" | "deny" | "cancel"));
                Ok(result)
            })();
            let (reply, stop) = match result {
                Ok((v, stop)) => (json!({"value":v}), stop),
                Err(e) => (json!({"error":e}), false),
            };
            if let Err(e) = write_frame(&stream, &reply) {
                if e == "daemon frame exceeds 4 MiB" {
                    let _ = write_frame(&stream, &json!({"error":e}));
                }
            }
            if stop {
                return Ok(());
            }
        }
        while wake && active < concurrency {
            if !launch(&mut store, &loc.workspace, &tx, tool_first)? {
                wake = false;
                break;
            }
            active += 1;
            tool_first = !tool_first;
        }
        thread::sleep(Duration::from_millis(25));
    }
}
