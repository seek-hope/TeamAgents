//! teamagents-core: stdio JSON-lines service around `teamagents_core::server`.
//!
//! ponytail: plain stdin/stdout beats a socket or napi bridge here — callers
//! spawn this binary and speak newline-delimited JSON. Each session gets its
//! own SQLite connection on the same WAL file (busy_timeout set).

use serde_json::{json, Value as Json};
use std::io::{BufRead, Write};
use teamagents_core::server::Server;

fn handle(server: &mut Server, req: &Json) -> Json {
    let id = req.get("id").cloned().unwrap_or(Json::Null);
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Json::Null);
    match server.dispatch(method, &params) {
        Ok(result) => json!({"id": id, "result": result}),
        Err(e) => json!({"id": id, "error": e}),
    }
}

fn main() {
    let db = std::env::args().nth(1).unwrap_or_else(|| ":memory:".into());
    let mut server = Server::new(db);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Json>(&line) {
            Ok(req) => handle(&mut server, &req),
            Err(e) => json!({"id": null, "error": format!("bad json: {e}")}),
        };
        let mut out = stdout.lock();
        let _ = writeln!(out, "{reply}");
        let _ = out.flush();
    }
}
