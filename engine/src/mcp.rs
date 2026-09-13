//! Minimal MCP stdio client (tools.py::_load_service, plan §12.1).
//!
//! One short-lived session per service: connect, initialize, tools/list, then
//! tools/call on demand — same trade-off the Python build documents (no leaked
//! processes; switch to a long-lived session per service if latency matters).

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
    child: Mutex<Option<Child>>,
    stdin: Mutex<ChildStdin>,
    pending: Pending,
    next_id: AtomicU64,
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
            // the server's stderr is the engine's stderr (the Python SDK forwards
            // it to errlog): piping it without draining deadlocks a chatty server
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
            child: Mutex::new(Some(child)),
            stdin: Mutex::new(stdin),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        });
        let this = client.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(message) = serde_json::from_str::<Json>(&line) else { continue };
                let Some(id) = message.get("id").and_then(|v| v.as_u64()) else { continue };
                let slot = this.pending.lock().unwrap().remove(&id);
                if let Some(tx) = slot {
                    let result = match message.get("error") {
                        Some(error) if !error.is_null() => Err(format!("{error}")),
                        _ => Ok(message.get("result").cloned().unwrap_or(Json::Null)),
                    };
                    let _ = tx.send(result);
                }
            }
            let pending: Vec<_> = this.pending.lock().unwrap().drain().collect();
            for (_, tx) in pending {
                let _ = tx.send(Err("MCP server exited".into()));
            }
        });
        client.call("initialize", json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "teamagents", "version": env!("CARGO_PKG_VERSION")},
        }), 60_000)?;
        client.notify("notifications/initialized", json!({}))?;
        Ok(client)
    }

    pub fn call(&self, method: &str, params: Json, timeout_ms: u64) -> Result<Json, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.write(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
            Ok(result) => result,
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!("MCP {method} timed out"))
            }
        }
    }

    fn notify(&self, method: &str, params: Json) -> Result<(), String> {
        self.write(json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn write(&self, message: Json) -> Result<(), String> {
        let mut stdin = self.stdin.lock().unwrap();
        writeln!(stdin, "{message}").map_err(|e| e.to_string())?;
        stdin.flush().map_err(|e| e.to_string())
    }

    /// tools/list → (name, description, inputSchema) rows.
    pub fn tools(&self) -> Result<Vec<Json>, String> {
        let reply = self.call("tools/list", json!({}), 60_000)?;
        Ok(reply.get("tools").and_then(|v| v.as_array()).cloned().unwrap_or_default())
    }

    /// tools/call → the text content of the result (or an error string).
    pub fn call_tool(&self, name: &str, args: &Json) -> Result<Json, String> {
        let reply = self.call("tools/call", json!({"name": name, "arguments": args}), 120_000)?;
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
        let child = self.child.lock().unwrap().take();
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            if let Some(mut child) = child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}
