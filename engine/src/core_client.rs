//! In-process client for the authoritative core.
//!
//! Same JSON method surface the `teamagents-core` binary exposes over stdio
//! (core::server::dispatch) — one implementation, two transports. DB work is
//! serialized behind one mutex (SQLite is single-writer anyway).

use serde_json::{json, Value as Json};
use std::sync::{Arc, Mutex};
use teamagents_core::control::MidTurnPush;
use teamagents_core::models::{ActionKind, Receipt, TeamAction, TurnRun};
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
        self.server.lock().map_err(|_| "core lock poisoned".to_string())?.dispatch(method, &params)
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

    pub(crate) fn restore_queued_runs(&self, run_ids: &[String]) -> Result<Vec<TurnRun>, String> {
        self.server
            .lock()
            .map_err(|_| "core lock poisoned".to_string())?
            .control_for(&self.session_id)?
            .restore_queued_runs(run_ids)
    }

    pub(crate) fn submit_prepared_topology(
        &self,
        action: &TeamAction,
        preparation: Result<&[Json], &str>,
    ) -> Result<Receipt, String> {
        self.server
            .lock()
            .map_err(|_| "core lock poisoned".to_string())?
            .control_for(&self.session_id)?
            .submit(teamagents_core::control::ActionSubmission::PreparedTopology { action, preparation })
    }

    /// Read-only preflight before the engine prepares persistent model profiles.
    /// None goes unchanged to submit, which records refusals and rechecks races.
    /// This uses the linked core without adding another wire mutation endpoint.
    pub(crate) fn topology_operations_for_preparation(&self, action: &TeamAction) -> Result<Option<Vec<Json>>, String> {
        if action.kind != ActionKind::ApplyTopologyPatch {
            return Err("topology preparation requires apply_topology_patch".into());
        }
        let mut server = self.server.lock().map_err(|_| "core lock poisoned".to_string())?;
        let ctl = server.control_for(&self.session_id)?;
        if ctl.store.get_action_receipt(&action.action_id).map_err(|e| e.to_string())?.is_some() {
            // Submit checks the original request hash, including collisions;
            // either outcome must bypass preparation and its file side effects.
            return Ok(None);
        }
        let spec = ctl.store.load_team_spec(&self.session_id, None).map_err(|e| e.to_string())?;
        if ctl.validate(action, &spec).is_some() || action.payload["reject"] == true {
            return Ok(None);
        }
        if let Some(operations) = action.payload["operations"].as_array().filter(|ops| !ops.is_empty()) {
            return Ok(Some(operations.clone()));
        }
        let patch_id = action.payload["patch_id"].as_str().ok_or("validated patch is missing its patch_id")?;
        let patch = ctl
            .store
            .get_patch_for_session(&self.session_id, patch_id)
            .map_err(|e| e.to_string())?
            .ok_or("validated patch no longer exists")?;
        Ok(Some(patch.operations))
    }

    /// Buffered payloads are not authority. Resolve their durable IDs again
    /// immediately before a backend puts them into its context.
    pub fn revalidate_inbox(&self, agent_id: &str, mut view: Json) -> Result<Json, String> {
        let Some(items) = view.get("inbox_delta").and_then(|v| v.as_array()) else { return Ok(view) };
        if items.is_empty() {
            return Ok(view);
        }
        let ids: Vec<i64> = items
            .iter()
            .map(|item| item["delivery_id"].as_i64().ok_or("buffered input lacks a delivery id"))
            .collect::<Result<_, _>>()?;
        let reply = self.call_in_session("delivery_items", json!({"agent_id": agent_id, "delivery_ids": ids}))?;
        let current = reply["items"].as_array().ok_or("invalid delivery projection")?.clone();
        view["delivery_ids"] = json!(current.iter().filter_map(|i| i["delivery_id"].as_i64()).collect::<Vec<_>>());
        view["inbox_delta"] = json!(current);
        Ok(view)
    }

    pub fn state(&self) -> Result<Json, String> {
        self.call_in_session("state", json!({}))
    }

    /// Use the same core operation as the wire RPC, with no fallible decoding
    /// after the committed batches have been removed from its memory buffer.
    pub(crate) fn drain_mid_turn(&self) -> Result<Vec<MidTurnPush>, String> {
        self.server
            .lock()
            .map_err(|_| "core lock poisoned".to_string())?
            .control_for(&self.session_id)?
            .drain_mid_turn_pushes()
    }

    /// `state` without the events tail (P2-9): for hot paths that never read
    /// `events`, so the core skips serializing up to 1000 events per call.
    pub fn state_brief(&self) -> Result<Json, String> {
        self.call_in_session("state", json!({"include_events": false}))
    }

    pub(crate) fn run_started_ids(&self, run_ids: &[String]) -> Result<std::collections::HashSet<String>, String> {
        self.server
            .lock()
            .map_err(|_| "core lock poisoned".to_string())?
            .control_for(&self.session_id)?
            .store
            .run_started_ids(&self.session_id, run_ids)
            .map_err(|error| error.to_string())
    }
}
