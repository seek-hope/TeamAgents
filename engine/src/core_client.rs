//! In-process client for the authoritative core.
//!
//! Same JSON method surface the `teamagents-core` binary exposes over stdio
//! (core::server::dispatch) — one implementation, two transports. DB work is
//! serialized behind one mutex (SQLite is single-writer anyway).

use serde_json::{json, Value as Json};
use std::sync::{Arc, Mutex};
use teamagents_core::models::{Receipt, TeamAction};
use teamagents_core::server::Server;

pub struct CoreClient {
    server: Mutex<Server>,
    pub session_id: String,
}

impl CoreClient {
    pub fn open(db: &str, session_id: &str) -> Result<Arc<Self>, String> {
        let server = Server::new(db);
        Ok(Arc::new(Self { server: Mutex::new(server), session_id: session_id.to_string() }))
    }

    /// `method` + params, exactly as the stdio protocol defines them.
    pub fn call(&self, method: &str, params: Json) -> Result<Json, String> {
        self.server
            .lock()
            .map_err(|_| "core lock poisoned".to_string())?
            .dispatch(method, &params)
    }

    /// Call with this session's id injected (the worker's `call` passthrough).
    pub fn call_in_session(&self, method: &str, mut params: Json) -> Result<Json, String> {
        let object = params.as_object_mut().ok_or("params must be an object")?;
        object.insert("session_id".into(), json!(self.session_id));
        self.call(method, params)
    }

    pub fn submit(&self, action: &TeamAction) -> Result<Receipt, String> {
        let value = serde_json::to_value(action).map_err(|e| e.to_string())?;
        let result = self.call("submit", value)?;
        serde_json::from_value(result).map_err(|e| format!("bad receipt: {e}"))
    }

    pub fn state(&self) -> Result<Json, String> {
        self.call_in_session("state", json!({}))
    }
}
