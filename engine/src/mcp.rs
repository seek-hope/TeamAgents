//! Minimal MCP client: stdio + streamable HTTP transports (plan §12.1).
//!
//! One short-lived session per service: connect, initialize, tools/list, then
//! tools/call on demand (no leaked processes; switch to a long-lived session
//! per service if latency matters).

use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
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
        pending: Pending,
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
        let mut cmd = Command::new(command);
        cmd.env_clear();
        for key in INHERITED_ENV {
            if let Ok(value) = std::env::var(key) {
                if !value.starts_with("()") {
                    cmd.env(key, value);
                }
            }
        }
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // the server's stderr is the engine's stderr: piping it without
            // draining deadlocks a chatty server
            // ponytail: server logs land on the engine's stderr; drain into a
            // bounded buffer if the TUI needs that stream clean.
            .stderr(Stdio::inherit());
        for (key, value) in env {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn().map_err(|e| format!("cannot start MCP server {command:?}: {e}"))?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let client = Arc::new(Self {
            transport: Transport::Stdio {
                child: Mutex::new(Some(child)),
                stdin: Mutex::new(stdin),
                pending: Mutex::new(HashMap::new()),
            },
            next_id: AtomicU64::new(1),
            startup_ms: 60_000,
            tool_ms: 120_000,
        });
        let this = client.clone();
        std::thread::spawn(move || {
            let Transport::Stdio { pending, .. } = &this.transport else { return };
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
        // P2-5: the reader thread holds an Arc, so Drop never fires on its own;
        // a failed handshake must kill+wait the server before returning Err.
        if let Err(e) = client.call("initialize", json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "teamagents", "version": env!("CARGO_PKG_VERSION")},
        }), client.startup_ms) {
            client.close();
            return Err(e);
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
        let client = Arc::new(Self {
            transport: Transport::Http {
                url: url.to_string(),
                token,
                session: Mutex::new(None),
                protocol: Mutex::new(None),
            },
            next_id: AtomicU64::new(1),
            startup_ms: startup_timeout_s.max(1) * 1000,
            tool_ms: tool_timeout_s.max(1) * 1000,
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
                write_line(stdin, &body)?;
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
    let mut request = ureq::post(url)
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
