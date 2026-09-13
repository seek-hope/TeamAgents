//! Minimal stdio MCP server for tests (the former tests/mcp_echo_server.py):
//! one `echo` tool over newline-delimited JSON-RPC on stdin/stdout.
//! `--noisy-stderr <bytes>` writes that many bytes to stderr before serving,
//! which is how the stdio client's stderr handling is exercised.

use serde_json::{json, Value as Json};
use std::io::{BufRead, Write};

fn main() {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--noisy-stderr" {
            let bytes: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(70_000);
            let mut stderr = std::io::stderr().lock();
            let chunk = vec![b'x'; bytes.min(8192)];
            let mut written = 0;
            while written < bytes {
                let n = chunk.len().min(bytes - written);
                let _ = stderr.write_all(&chunk[..n]);
                written += n;
            }
            let _ = stderr.flush();
        }
    }

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Json>(&line) else { continue };
        let Some(id) = message.get("id").cloned() else { continue }; // notification
        let method = message.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let params = message.get("params").cloned().unwrap_or(Json::Null);
        let result = match method {
            "initialize" => json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "fake-mcp-server", "version": env!("CARGO_PKG_VERSION")},
            }),
            "tools/list" => json!({"tools": [{
                "name": "echo",
                "description": "Echo the given text N times.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"text": {"type": "string"}, "times": {"type": "integer"}},
                    "required": ["text"],
                },
            }]}),
            "tools/call" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
                if name != "echo" {
                    json!({"content": [{"type": "text", "text": format!("unknown tool {name}")}], "isError": true})
                } else {
                    let text = arguments.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    let times = arguments.get("times").and_then(|v| v.as_i64()).unwrap_or(1).max(0) as usize;
                    let echoed = std::iter::repeat(text).take(times).collect::<Vec<_>>().join(" ");
                    json!({"content": [{"type": "text", "text": echoed}]})
                }
            }
            _ => json!({}),
        };
        let _ = writeln!(out, "{}", json!({"jsonrpc": "2.0", "id": id, "result": result}));
        let _ = out.flush();
    }
}
