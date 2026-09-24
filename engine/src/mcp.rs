//! Minimal MCP client: stdio + streamable HTTP transports (plan §12.1).
//!
//! One connection per bound member service, retained until its runner closes.
//! Explicit close and failed initialization reap the owned stdio process group.

use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Pending = Mutex<HashMap<u64, Sender<Result<Json, String>>>>;

/// Only these variables are inherited by a stdio MCP server
/// (mcp.client.stdio.get_default_environment: HOME/LOGNAME/PATH/SHELL/TERM/USER);
/// binding.env is layered on top. Model API keys must never leak to a server.
const INHERITED_ENV: &[&str] = &["HOME", "LOGNAME", "PATH", "SHELL", "TERM", "USER"];

pub struct McpClient {
    transport: Transport,
    next_id: AtomicU64,
    startup_ms: u64,
    tool_ms: u64,
    closed: AtomicBool,
    /// The member's workspace, answered to `roots/list` requests. The caller sets
    /// it after connecting (the transport itself does not know the member root).
    workspace: Arc<Mutex<Option<std::path::PathBuf>>>,
}

enum Transport {
    Stdio {
        child: Mutex<Option<Child>>,
        stdin: Arc<Mutex<ChildStdin>>,
        pending: Arc<Pending>,
    },
    Http {
        url: String,
        token: Option<String>,
        // shared with the push reader thread, which may need to answer a
        // server-initiated request over the same session
        session: Arc<Mutex<Option<String>>>,
        protocol: Arc<Mutex<Option<String>>>,
        stream: Mutex<Option<PushStream>>,
    },
}

impl McpClient {
    /// Binding authorizes the service; host execution requires an explicit mode.
    #[expect(
        clippy::too_many_arguments,
        reason = "Preserve the existing transport API and its explicit isolation options."
    )]
    pub fn connect_stdio_in(
        command: &str,
        args: &[String],
        env: &[(String, String)],
        root: &Path,
        mode: &str,
        network: bool,
        startup_timeout_s: u64,
        tool_timeout_s: u64,
    ) -> Result<Arc<Self>, String> {
        let root = root.canonicalize().map_err(|e| format!("MCP workspace unavailable: {e}"))?;
        let mut cmd = match mode {
            "workspace" => {
                if !crate::tools::bwrap_available() {
                    return Err("IsolationUnavailable: MCP workspace execution requires bwrap".into());
                }
                let mut argv = crate::tools::bwrap_argv(&root, network, "", None);
                // Reuse the shell sandbox, replacing only its Bash invocation.
                argv.truncate(argv.len() - 3);
                let mut cmd = Command::new(&argv[0]);
                cmd.args(&argv[1..]).arg(command).args(args);
                cmd
            }
            "host" => {
                let mut cmd = Command::new(command);
                cmd.args(args);
                cmd
            }
            other => return Err(format!("unsupported MCP execution mode {other:?}; use workspace or host")),
        };
        cmd.current_dir(&root);
        cmd.env_clear();
        for key in INHERITED_ENV {
            if let Ok(value) = std::env::var(key) {
                if !value.starts_with("()") {
                    cmd.env(key, value);
                }
            }
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // the server's stderr is the engine's stderr: piping it without
            // draining deadlocks a chatty server
            // ponytail: server logs land on the engine's stderr; drain into a
            // bounded buffer if the TUI needs that stream clean.
            .stderr(Stdio::inherit());
        for (key, value) in env {
            cmd.env(key, value);
        }
        if mode == "workspace" {
            cmd.env("HOME", crate::tools::sandbox_home(false));
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().map_err(|e| format!("cannot start MCP server {command:?}: {e}"))?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let stdin = Arc::new(Mutex::new(stdin));
        let reader_stdin = stdin.clone();
        let workspace: Arc<Mutex<Option<std::path::PathBuf>>> = Arc::new(Mutex::new(None));
        let reader_workspace = workspace.clone();
        let client = Arc::new(Self {
            transport: Transport::Stdio {
                child: Mutex::new(Some(child)),
                stdin: stdin.clone(),
                pending: pending.clone(),
            },
            next_id: AtomicU64::new(1),
            startup_ms: startup_timeout_s.max(1).saturating_mul(1000),
            tool_ms: tool_timeout_s.max(1).saturating_mul(1000),
            closed: AtomicBool::new(false),
            workspace,
        });
        // The reader owns only pending replies, so dropping the final client
        // can reap the server even when it never closes stdout. It also answers
        // server->client requests (roots/list); everything else is declined.
        let (stdin, workspace) = (reader_stdin, reader_workspace);
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(message) = serde_json::from_str::<Json>(&line) else { continue };
                let Some(id) = message.get("id").and_then(|v| v.as_u64()) else { continue };
                // a request has a method; a reply to one of ours does not
                if let Some(method) = message.get("method").and_then(|v| v.as_str()) {
                    if let Some(reply) = server_request_reply(id, method, workspace.lock().unwrap().as_deref()) {
                        let _ = write_line(&stdin, &reply);
                    }
                    continue;
                }
                let slot = pending.lock().unwrap().remove(&id);
                if let Some(tx) = slot {
                    let result = match message.get("error") {
                        Some(error) if !error.is_null() => Err(format!("{error}")),
                        _ => Ok(message.get("result").cloned().unwrap_or(Json::Null)),
                    };
                    let _ = tx.send(result);
                }
            }
            let drained: Vec<_> = pending.lock().unwrap().drain().collect();
            for (_, tx) in drained {
                let _ = tx.send(Err("MCP server exited".into()));
            }
        });
        // A failed handshake must kill+wait the server before returning Err.
        if let Err(e) = client.call(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {"roots": {}},
                "clientInfo": {"name": "teamagents", "version": env!("CARGO_PKG_VERSION")},
            }),
            client.startup_ms,
        ) {
            client.close();
            return Err(format!("MCP {mode} initialization failed: {e}"));
        }
        if let Err(e) = client.notify("notifications/initialized", json!({})) {
            client.close();
            return Err(e);
        }
        Ok(client)
    }

    /// Streamable HTTP transport (MCP 2025-06-18): every client message is a
    /// POST; the reply is one JSON document or an SSE stream of `data:` frames.
    /// `token` is read from the environment by the caller, never from config.
    pub fn connect_http(
        url: &str,
        token: Option<String>,
        startup_timeout_s: u64,
        tool_timeout_s: u64,
    ) -> Result<Arc<Self>, String> {
        let endpoint = url::Url::parse(url).map_err(|_| "invalid MCP HTTP endpoint".to_string())?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some()
        {
            return Err("MCP HTTP endpoint must be http(s), without credentials or a fragment".into());
        }
        let client = Arc::new(Self {
            transport: Transport::Http {
                url: url.to_string(),
                token,
                session: Arc::new(Mutex::new(None)),
                protocol: Arc::new(Mutex::new(None)),
                stream: Mutex::new(None),
            },
            next_id: AtomicU64::new(1),
            startup_ms: startup_timeout_s.max(1).saturating_mul(1000),
            tool_ms: tool_timeout_s.max(1).saturating_mul(1000),
            closed: AtomicBool::new(false),
            workspace: Arc::new(Mutex::new(None)),
        });
        let initialized = client.call(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {"roots": {}},
                "clientInfo": {"name": "teamagents", "version": env!("CARGO_PKG_VERSION")},
            }),
            client.startup_ms,
        )?;
        let version = initialized["protocolVersion"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or("MCP initialize response has no protocolVersion")?;
        if let Transport::Http { protocol, .. } = &client.transport {
            *protocol.lock().unwrap() = Some(version.to_string());
        }
        client.notify("notifications/initialized", json!({}))?;
        client.open_push_stream();
        Ok(client)
    }

    /// The server may push messages over a GET SSE stream (notifications,
    /// progress, or its own requests such as sampling/roots). Servers that do
    /// not support it answer 405 — then this is simply a no-op.
    fn open_push_stream(&self) {
        let Transport::Http { url, token, session, protocol, stream } = &self.transport else { return };
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout_connect(Duration::from_secs(2))
            .timeout_read(Duration::from_secs(1))
            .build();
        let mut request = agent.get(url).set("accept", "text/event-stream");
        if let Some(token) = token {
            request = request.set("authorization", &format!("Bearer {token}"));
        }
        if let Some(id) = session.lock().unwrap().clone() {
            request = request.set("mcp-session-id", &id);
        }
        if let Some(version) = protocol.lock().unwrap().clone() {
            request = request.set("mcp-protocol-version", &version);
        }
        let response = match request.call() {
            Ok(response) => response,
            // a server that refuses the stream (405/404) or is unreachable just
            // means "no push"; the transport still works over POST
            Err(error) => {
                eprintln!("MCP push stream unavailable: {error}");
                return;
            }
        };
        if !response.header("content-type").unwrap_or("").contains("text/event-stream") {
            return;
        }
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = response.into_reader();
        let reply =
            (url.clone(), token.clone(), session.clone(), protocol.clone(), stop.clone(), self.workspace.clone());
        let join = std::thread::spawn(move || read_push_stream(reader, reply));
        *stream.lock().unwrap() = Some(PushStream { stop, join: Some(join) });
    }

    /// The member's workspace, used to answer `roots/list`.
    pub fn set_workspace(&self, root: &Path) {
        *self.workspace.lock().unwrap() = Some(root.to_path_buf());
    }

    pub fn call(&self, method: &str, params: Json, timeout_ms: u64) -> Result<Json, String> {
        if self.closed.load(Ordering::SeqCst) {
            return Err("MCP client is closed".into());
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        match &self.transport {
            Transport::Stdio { stdin, pending, .. } => {
                let (tx, rx) = channel();
                {
                    let mut pending = pending.lock().unwrap();
                    // Serialize registration with close's drain, so no caller
                    // can park a new waiter after shutdown released the others.
                    if self.closed.load(Ordering::SeqCst) {
                        return Err("MCP client is closed".into());
                    }
                    pending.insert(id, tx);
                }
                if let Err(error) = write_line(stdin, &body) {
                    pending.lock().unwrap().remove(&id);
                    return Err(error);
                }
                match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
                    Ok(result) => result,
                    Err(_) => {
                        pending.lock().unwrap().remove(&id);
                        Err(format!("MCP {method} timed out"))
                    }
                }
            }
            Transport::Http { url, token, session, protocol, .. } => {
                let version = protocol.lock().unwrap().clone();
                let payload = http_roundtrip(url, token.as_deref(), session, version.as_deref(), &body, timeout_ms)?
                    .ok_or_else(|| format!("MCP {method} returned no response"))?;
                match payload.get("error") {
                    Some(error) if !error.is_null() => Err(format!("{error}")),
                    _ => Ok(payload.get("result").cloned().unwrap_or(Json::Null)),
                }
            }
        }
    }

    fn notify(&self, method: &str, params: Json) -> Result<(), String> {
        if self.closed.load(Ordering::SeqCst) {
            return Err("MCP client is closed".into());
        }
        let body = json!({"jsonrpc": "2.0", "method": method, "params": params});
        match &self.transport {
            Transport::Stdio { stdin, .. } => write_line(stdin, &body),
            Transport::Http { url, token, session, protocol, .. } => {
                let version = protocol.lock().unwrap().clone();
                http_roundtrip(url, token.as_deref(), session, version.as_deref(), &body, self.startup_ms)?;
                Ok(())
            }
        }
    }

    /// tools/list → (name, description, inputSchema) rows.
    pub fn tools(&self) -> Result<Vec<Json>, String> {
        let reply = self.call("tools/list", json!({}), self.startup_ms)?;
        Ok(reply.get("tools").and_then(|v| v.as_array()).cloned().unwrap_or_default())
    }

    /// tools/call → the text content of the result (or an error string).
    pub fn call_tool(&self, name: &str, args: &Json) -> Result<Json, String> {
        let reply = self.call("tools/call", json!({"name": name, "arguments": args}), self.tool_ms)?;
        let text = reply
            .get("content")
            .and_then(|v| v.as_array())
            .map(|items| {
                items.iter().filter_map(|item| item.get("text").and_then(|v| v.as_str())).collect::<Vec<_>>().join("\n")
            })
            .unwrap_or_default();
        if reply.get("isError").and_then(|v| v.as_bool()).unwrap_or(false) {
            return Err(if text.is_empty() { "MCP tool error".into() } else { text });
        }
        Ok(if text.is_empty() { reply } else { Json::String(text) })
    }

    pub fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        match &self.transport {
            Transport::Stdio { child, pending, .. } => {
                for (_, reply) in pending.lock().unwrap().drain() {
                    let _ = reply.send(Err("MCP client is closed".into()));
                }
                let child = child.lock().unwrap().take();
                if let Some(mut child) = child {
                    #[cfg(unix)]
                    {
                        // Match the Codex adapter's process-group ownership.
                        // A descendant may hold stdout open or ignore TERM;
                        // killing only the server would leave tools running.
                        let pgid = child.id();
                        for signal in ["TERM", "KILL"] {
                            let _ = Command::new("/bin/sh")
                                .args(["-c", &format!("kill -{signal} -{pgid}")])
                                .stderr(Stdio::null())
                                .status();
                            if signal == "TERM" {
                                std::thread::sleep(Duration::from_millis(100));
                            }
                        }
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            Transport::Http { url, token, session, protocol, stream } => {
                if let Some(handle) = stream.lock().unwrap().take() {
                    handle.stop.store(true, Ordering::SeqCst);
                    if let Some(join) = handle.join {
                        let deadline = std::time::Instant::now() + Duration::from_secs(3);
                        while !join.is_finished() && std::time::Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        if join.is_finished() {
                            let _ = join.join();
                        }
                    }
                }
                // DELETE terminates the session on the server (405 = unsupported)
                if let Some(id) = session.lock().unwrap().clone() {
                    let agent = ureq::AgentBuilder::new().redirects(0).build();
                    let mut request = agent.delete(url).timeout(Duration::from_secs(2)).set("mcp-session-id", &id);
                    if let Some(token) = token {
                        request = request.set("authorization", &format!("Bearer {token}"));
                    }
                    if let Some(version) = protocol.lock().unwrap().clone() {
                        request = request.set("mcp-protocol-version", &version);
                    }
                    match request.call() {
                        Ok(_) => {}
                        Err(ureq::Error::Status(code, _)) => {
                            if code != 405 {
                                eprintln!("MCP session delete returned {code}");
                            }
                        }
                        Err(error) => eprintln!("MCP session delete failed: {error}"),
                    }
                }
            }
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.close();
    }
}

struct PushStream {
    stop: Arc<std::sync::atomic::AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

/// Read the GET SSE stream: notifications go to the log, server-initiated
/// requests are answered with a JSON-RPC "not supported" error so a server never
/// waits forever on a capability this client does not implement.
type PushContext = (
    String,
    Option<String>,
    Arc<Mutex<Option<String>>>,
    Arc<Mutex<Option<String>>>,
    Arc<std::sync::atomic::AtomicBool>,
    Arc<Mutex<Option<std::path::PathBuf>>>,
);

fn read_push_stream(reader: Box<dyn std::io::Read + Send + Sync + 'static>, reply: PushContext) {
    let (url, token, session, protocol, stop, workspace) = reply;
    let mut reader = BufReader::new(reader);
    let mut frame = String::new();
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) => {}
            // the per-read timeout is how this thread learns to stop
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                continue
            }
            Err(_) => return,
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if let Some(data) = line.strip_prefix("data:") {
            if !frame.is_empty() {
                frame.push('\n');
            }
            frame.push_str(data.strip_prefix(' ').unwrap_or(data));
        }
        if !line.is_empty() || frame.is_empty() {
            continue;
        }
        let message: Json = match serde_json::from_str(&frame) {
            Ok(message) => message,
            Err(_) => {
                frame.clear();
                continue;
            }
        };
        frame.clear();
        let method = message.get("method").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(id) = message.get("id").and_then(Json::as_u64) {
            if let Some(body) = server_request_reply(id, method, workspace.lock().unwrap().as_deref()) {
                if let Err(error) = http_roundtrip(
                    &url,
                    token.as_deref(),
                    &session,
                    protocol.lock().unwrap().clone().as_deref(),
                    &body,
                    5_000,
                ) {
                    eprintln!("MCP push reply failed: {error}");
                }
            }
        } else if !method.is_empty() {
            eprintln!("MCP notification: {method}");
        }
    }
}

/// Reply to a server-initiated request. `roots` is the one capability this client
/// exposes; anything else is declined so the server never waits for an answer.
fn server_request_reply(id: u64, method: &str, workspace: Option<&Path>) -> Option<Json> {
    match method {
        "roots/list" => {
            let roots = match workspace {
                Some(root) => vec![json!({
                    "uri": format!("file://{}", root.to_string_lossy()),
                    "name": root.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "workspace".into()),
                })],
                None => vec![],
            };
            Some(json!({"jsonrpc": "2.0", "id": id, "result": {"roots": roots}}))
        }
        other => Some(json!({"jsonrpc": "2.0", "id": id,
                             "error": {"code": -32601, "message": format!("client does not support {other}")}})),
    }
}

fn write_line(stdin: &Mutex<ChildStdin>, message: &Json) -> Result<(), String> {
    let mut stdin = stdin.lock().unwrap();
    writeln!(stdin, "{message}").map_err(|e| e.to_string())?;
    stdin.flush().map_err(|e| e.to_string())
}

/// One POST of a JSON-RPC message; the response is a single JSON document or
/// an SSE stream. Returns None for a 202 (notification accepted, no body).
/// Remembers the server-issued `mcp-session-id` and sends it from then on.
fn http_roundtrip(
    url: &str,
    token: Option<&str>,
    session: &Mutex<Option<String>>,
    protocol: Option<&str>,
    body: &Json,
    timeout_ms: u64,
) -> Result<Option<Json>, String> {
    // A binding authorizes this endpoint only; never forward tokens or session
    // headers to a redirect target, including another endpoint on the same host.
    let agent = ureq::AgentBuilder::new().redirects(0).build();
    let mut request = agent
        .post(url)
        .timeout(Duration::from_millis(timeout_ms))
        .set("content-type", "application/json")
        .set("accept", "application/json, text/event-stream");
    if let Some(token) = token {
        request = request.set("authorization", &format!("Bearer {token}"));
    }
    if let Some(id) = session.lock().unwrap().clone() {
        request = request.set("mcp-session-id", &id);
    }
    if let Some(version) = protocol {
        request = request.set("mcp-protocol-version", version);
    }
    let response = request.send_string(&body.to_string()).map_err(|e| format!("MCP HTTP request failed: {e}"))?;
    if (300..400).contains(&response.status()) {
        return Err("MCP HTTP redirects are not allowed; configure the final endpoint explicitly".into());
    }
    if let Some(id) = response.header("mcp-session-id") {
        *session.lock().unwrap() = Some(id.to_string());
    }
    if response.status() == 202 {
        return Ok(None);
    }
    let content_type = response.header("content-type").unwrap_or("").to_string();
    if content_type.contains("text/event-stream") {
        let stream = response.into_string().map_err(|e| format!("MCP HTTP bad SSE body: {e}"))?;
        return sse_json(&stream, &body["id"])
            .map(Some)
            .ok_or_else(|| "MCP HTTP SSE stream carried no matching JSON-RPC response".into());
    }
    let payload: Json = response.into_json().map_err(|e| format!("MCP HTTP bad json: {e}"))?;
    if body.get("id").is_some() && payload.get("id") != body.get("id") {
        return Err("MCP HTTP response id does not match request".into());
    }
    Ok(Some(payload))
}

/// Blank lines delimit events; all data fields in one event form one payload.
fn sse_json(body: &str, id: &Json) -> Option<Json> {
    let normalized = body.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n");
    for event in normalized.split_inclusive("\n\n") {
        if !event.ends_with("\n\n") {
            continue;
        }
        let data = event
            .lines()
            .filter_map(|line| {
                if line == "data" {
                    return Some("");
                }
                line.strip_prefix("data:").map(|s| s.strip_prefix(' ').unwrap_or(s))
            })
            .collect::<Vec<_>>()
            .join("\n");
        if let Ok(message) = serde_json::from_str::<Json>(&data) {
            if message.get("id") == Some(id) && (message.get("result").is_some() || message.get("error").is_some()) {
                return Some(message);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn stdio_workspace_isolates_files_network_and_preserves_argv() {
        let dir = std::env::temp_dir().join(format!("ta-mcp-policy-{}", uuid::Uuid::new_v4()));
        let root = dir.join("member");
        std::fs::create_dir_all(&root).unwrap();
        let outside = dir.join("private");
        std::fs::write(&outside, "private").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let literal = "spaces ; $(touch injected) ' quote";
        let script = r#"
import json, os, socket, sys
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request: continue
    if request['method'] == 'initialize':
        result = {'protocolVersion': '2025-06-18'}
    else:
        with open('created', 'w') as f: f.write('member output')
        sock = socket.socket()
        sock.settimeout(0.5)
        network = sock.connect_ex(('127.0.0.1', int(sys.argv[2]))) == 0
        sock.close()
        result = {'cwd': os.getcwd(), 'home': os.environ['HOME'], 'home_exists': os.path.isdir(os.environ['HOME']), 'outside': os.path.exists(sys.argv[1]), 'network': network, 'literal': sys.argv[3]}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#;
        let args = vec![
            "-u".into(),
            "-c".into(),
            script.into(),
            outside.display().to_string(),
            listener.local_addr().unwrap().port().to_string(),
            literal.into(),
        ];
        if !crate::tools::bwrap_available() {
            // never degrade to unsandboxed execution: the workspace mode must fail
            // outright (CI has no bwrap)
            assert!(
                McpClient::connect_stdio_in("/usr/bin/python3", &args, &[], &root, "workspace", false, 2, 3).is_err()
            );
            std::fs::remove_dir_all(dir).unwrap();
            return;
        }
        for (mode, network, visible) in [("workspace", false, false), ("workspace", true, false), ("host", false, true)]
        {
            let client =
                McpClient::connect_stdio_in("/usr/bin/python3", &args, &[], &root, mode, network, 2, 3).unwrap();
            assert_eq!((client.startup_ms, client.tool_ms), (2000, 3000));
            let response = client.call("probe", json!({}), 3000).unwrap();
            assert_eq!(response["cwd"], root.display().to_string());
            assert_eq!(response["outside"], visible);
            assert_eq!(response["network"], network || mode == "host");
            assert_eq!(response["literal"], literal);
            if mode == "workspace" {
                assert_ne!(response["home"], root.display().to_string());
                assert_eq!(response["home_exists"], true);
            }
            assert!(root.join("created").is_file());
            assert!(!root.join("injected").exists());
            let Transport::Stdio { child, .. } = &client.transport else { unreachable!() };
            let pid = child.lock().unwrap().as_ref().unwrap().id();
            drop(client);
            assert!(!Path::new(&format!("/proc/{pid}")).exists(), "last client drop must reap the server");
        }
        assert!(McpClient::connect_stdio_in("sh", &[], &[], &root, "automatic", false, 1, 1).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn roots_list_is_answered_and_other_requests_are_declined() {
        let reply = server_request_reply(7, "roots/list", Some(Path::new("/tmp/member/work"))).unwrap();
        assert_eq!(reply["id"], 7);
        assert_eq!(reply["result"]["roots"][0]["uri"], "file:///tmp/member/work");
        assert_eq!(reply["result"]["roots"][0]["name"], "work");
        // no workspace set: an empty root list, not an error
        let empty = server_request_reply(8, "roots/list", None).unwrap();
        assert_eq!(empty["result"]["roots"].as_array().unwrap().len(), 0);
        // anything else is declined so the server never waits
        let declined = server_request_reply(9, "sampling/createMessage", Some(Path::new("/tmp"))).unwrap();
        assert_eq!(declined["error"]["code"], -32601);
        assert!(declined["error"]["message"].as_str().unwrap().contains("sampling/createMessage"));
    }

    #[test]
    fn http_rejects_credentials_and_never_follows_redirects() {
        for endpoint in ["file:///tmp/mcp", "http://user:secret@localhost/mcp", "http://localhost/mcp#fragment"] {
            assert!(McpClient::connect_http(endpoint, None, 1, 1).is_err());
        }
        let redirected = TcpListener::bind("127.0.0.1:0").unwrap();
        redirected.set_nonblocking(true).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let target = format!("http://{}/stolen", redirected.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            // a real server consumes the request body; leaving it unread makes the
            // socket close with RST and the client sees ECONNRESET instead of the
            // 302. That race made this test flaky.
            let mut body = vec![0u8; length];
            use std::io::Read as _;
            reader.read_exact(&mut body).unwrap();
            write!(
                stream,
                "HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            stream.flush().unwrap();
        });
        let error = McpClient::connect_http(&endpoint, Some("private-token".into()), 1, 1).err().unwrap();
        server.join().unwrap();
        assert!(error.contains("redirect"), "{error}");
        assert_eq!(redirected.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }
}
