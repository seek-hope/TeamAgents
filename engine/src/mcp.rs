//! Minimal MCP client: stdio + streamable HTTP transports (plan §12.1).
//!
//! One short-lived session per service: connect, initialize, tools/list, then
//! tools/call on demand (no leaked processes; switch to a long-lived session
//! per service if latency matters).

use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
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
}

enum Transport {
    Stdio {
        child: Mutex<Option<Child>>,
        stdin: Mutex<ChildStdin>,
        pending: Arc<Pending>,
    },
    Http {
        url: String,
        token: Option<String>,
        session: Mutex<Option<String>>,
        protocol: Mutex<Option<String>>,
    },
}

impl McpClient {
    pub fn connect_stdio(command: &str, args: &[String], env: &[(String, String)]) -> Result<Arc<Self>, String> {
        let root = std::env::current_dir().map_err(|e| e.to_string())?;
        Self::connect_stdio_in(command, args, env, &root, "workspace", false, 60, 120)
    }

    /// Binding authorizes the service; host execution requires an explicit mode.
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
            cmd.env("HOME", &root);
        }
        let mut child = cmd.spawn().map_err(|e| format!("cannot start MCP server {command:?}: {e}"))?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let client = Arc::new(Self {
            transport: Transport::Stdio {
                child: Mutex::new(Some(child)),
                stdin: Mutex::new(stdin),
                pending: pending.clone(),
            },
            next_id: AtomicU64::new(1),
            startup_ms: startup_timeout_s.max(1).saturating_mul(1000),
            tool_ms: tool_timeout_s.max(1).saturating_mul(1000),
        });
        // The reader owns only pending replies, so dropping the final client
        // can reap the server even when it never closes stdout.
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(message) = serde_json::from_str::<Json>(&line) else { continue };
                let Some(id) = message.get("id").and_then(|v| v.as_u64()) else { continue };
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
        if let Err(e) = client.call("initialize", json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "teamagents", "version": env!("CARGO_PKG_VERSION")},
        }), client.startup_ms) {
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
    /// ponytail: no server-initiated messages (the standalone GET SSE stream is
    /// never opened) and no session-resume DELETE; a server that pushes
    /// notifications or needs explicit session teardown gets those when one
    /// shows up in practice.
    pub fn connect_http(
        url: &str,
        token: Option<String>,
        startup_timeout_s: u64,
        tool_timeout_s: u64,
    ) -> Result<Arc<Self>, String> {
        let endpoint = url::Url::parse(url).map_err(|_| "invalid MCP HTTP endpoint".to_string())?;
        if !matches!(endpoint.scheme(), "http" | "https") || endpoint.host_str().is_none()
            || !endpoint.username().is_empty() || endpoint.password().is_some() || endpoint.fragment().is_some()
        {
            return Err("MCP HTTP endpoint must be http(s), without credentials or a fragment".into());
        }
        let client = Arc::new(Self {
            transport: Transport::Http {
                url: url.to_string(),
                token,
                session: Mutex::new(None),
                protocol: Mutex::new(None),
            },
            next_id: AtomicU64::new(1),
            startup_ms: startup_timeout_s.max(1).saturating_mul(1000),
            tool_ms: tool_timeout_s.max(1).saturating_mul(1000),
        });
        let initialized = client.call("initialize", json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "teamagents", "version": env!("CARGO_PKG_VERSION")},
        }), client.startup_ms)?;
        let version = initialized["protocolVersion"].as_str().filter(|v| !v.is_empty())
            .ok_or("MCP initialize response has no protocolVersion")?;
        if let Transport::Http { protocol, .. } = &client.transport {
            *protocol.lock().unwrap() = Some(version.to_string());
        }
        client.notify("notifications/initialized", json!({}))?;
        Ok(client)
    }

    pub fn call(&self, method: &str, params: Json, timeout_ms: u64) -> Result<Json, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        match &self.transport {
            Transport::Stdio { stdin, pending, .. } => {
                let (tx, rx) = channel();
                pending.lock().unwrap().insert(id, tx);
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
            Transport::Http { url, token, session, protocol } => {
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
        let body = json!({"jsonrpc": "2.0", "method": method, "params": params});
        match &self.transport {
            Transport::Stdio { stdin, .. } => write_line(stdin, &body),
            Transport::Http { url, token, session, protocol } => {
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
                items
                    .iter()
                    .filter_map(|item| item.get("text").and_then(|v| v.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        if reply.get("isError").and_then(|v| v.as_bool()).unwrap_or(false) {
            return Err(if text.is_empty() { "MCP tool error".into() } else { text });
        }
        Ok(if text.is_empty() { reply } else { Json::String(text) })
    }

    pub fn close(&self) {
        if let Transport::Stdio { child, .. } = &self.transport {
            let child = child.lock().unwrap().take();
            if let Some(mut child) = child {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.close();
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
    let mut request = agent.post(url)
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
        return sse_json(&stream, &body["id"]).map(Some).ok_or_else(|| "MCP HTTP SSE stream carried no matching JSON-RPC response".into());
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
        if !event.ends_with("\n\n") { continue; }
        let data = event.lines().filter_map(|line| {
            if line == "data" { return Some(""); }
            line.strip_prefix("data:").map(|s| s.strip_prefix(' ').unwrap_or(s))
        }).collect::<Vec<_>>().join("\n");
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
        result = {'cwd': os.getcwd(), 'home': os.environ['HOME'], 'outside': os.path.exists(sys.argv[1]), 'network': network, 'literal': sys.argv[3]}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#;
        let args = vec!["-u".into(), "-c".into(), script.into(), outside.display().to_string(),
            listener.local_addr().unwrap().port().to_string(), literal.into()];
        for (mode, network, visible) in [("workspace", false, false), ("workspace", true, false), ("host", false, true)] {
            let client = McpClient::connect_stdio_in("/usr/bin/python3", &args, &[], &root, mode, network, 2, 3).unwrap();
            assert_eq!((client.startup_ms, client.tool_ms), (2000, 3000));
            let response = client.call("probe", json!({}), 3000).unwrap();
            assert_eq!(response["cwd"], root.display().to_string());
            assert_eq!(response["outside"], visible);
            assert_eq!(response["network"], network || mode == "host");
            assert_eq!(response["literal"], literal);
            if mode == "workspace" { assert_eq!(response["home"], root.display().to_string()); }
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
                if line == "\r\n" || line.is_empty() { break; }
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
            write!(stream, "HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            stream.flush().unwrap();
        });
        let error = McpClient::connect_http(&endpoint, Some("private-token".into()), 1, 1).err().unwrap();
        server.join().unwrap();
        assert!(error.contains("redirect"), "{error}");
        assert_eq!(redirected.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }
}
