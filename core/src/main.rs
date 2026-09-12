//! teamagents-core: stdio JSON-lines service. One JSON request per line:
//!   {"id": N, "method": "submit", "params": {TeamAction}}
//! One JSON response per line:
//!   {"id": N, "result": ...} or {"id": N, "error": "..."}
//!
//! ponytail: plain stdin/stdout beats a socket or napi bridge here — the TS
//! side spawns this binary and speaks newline-delimited JSON. Each session
//! gets its own SQLite connection on the same WAL file (busy_timeout set).

use serde_json::json;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use teamagents_core::control::Control;
use teamagents_core::models::*;
use teamagents_core::storage::Store;

struct Server {
    db: String,
    controls: HashMap<String, Control>,
}

impl Server {
    fn control_for(&mut self, sid: &str) -> Result<&mut Control, String> {
        if !self.controls.contains_key(sid) {
            let store = if self.db == ":memory:" {
                Store::open_memory()
            } else {
                Store::open(std::path::Path::new(&self.db))
            }
            .map_err(|e| format!("open {}: {e}", self.db))?;
            self.controls.insert(sid.to_string(), Control::new(store, sid));
        }
        Ok(self.controls.get_mut(sid).expect("just inserted"))
    }

    fn handle(&mut self, req: &Json) -> Json {
        let id = req.get("id").cloned().unwrap_or(Json::Null);
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(Json::Null);
        match method {
            "ping" => json!({"id": id, "result": {"core": env!("CARGO_PKG_VERSION")}}),
            "create_session" => self.create_session(id, &params),
            "submit" => match serde_json::from_value::<TeamAction>(params) {
                Ok(action) => {
                    let sid = action.session_id.clone();
                    match self.control_for(&sid) {
                        Ok(ctl) => json!({"id": id, "result": ctl.submit(&action)}),
                        Err(e) => json!({"id": id, "error": e}),
                    }
                }
                Err(e) => json!({"id": id, "error": format!("bad action: {e}")}),
            },
            _ => json!({"id": id, "error": format!("unknown method {method:?}")}),
        }
    }

    fn create_session(&mut self, id: Json, params: &Json) -> Json {
        let sid = params.get("session_id").and_then(|v| v.as_str());
        let cwd = params.get("cwd").and_then(|v| v.as_str());
        let (Some(sid), Some(cwd)) = (sid, cwd) else {
            return json!({"id": id, "error": "create_session needs session_id and cwd"});
        };
        let mode = params.get("permissions_mode").and_then(|v| v.as_str()).unwrap_or("approved_scope");
        match self.control_for(sid) {
            Ok(ctl) => match ctl.store.create_session(sid, cwd, mode) {
                Ok(()) => json!({"id": id, "result": {"session_id": sid}}),
                Err(e) => json!({"id": id, "error": format!("create_session: {e}")}),
            },
            Err(e) => json!({"id": id, "error": e}),
        }
    }
}

fn main() {
    let db = std::env::args().nth(1).unwrap_or_else(|| ":memory:".into());
    let mut server = Server { db, controls: HashMap::new() };
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Json>(&line) {
            Ok(req) => server.handle(&req),
            Err(e) => json!({"id": null, "error": format!("bad json: {e}")}),
        };
        let mut out = stdout.lock();
        let _ = writeln!(out, "{reply}");
        let _ = out.flush();
    }
}
