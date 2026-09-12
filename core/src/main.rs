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
use teamagents_core::control::{Control, EventDraft, TurnOutcome};
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
            "save_spec" => self.with(id, &params, |ctl, p| {
                let spec: TeamSpec = serde_json::from_value(p.get("spec").cloned().unwrap_or(Json::Null))
                    .map_err(|e| format!("bad spec: {e}"))?;
                spec.validate().map_err(|e| format!("invalid spec: {e}"))?;
                let rev = ctl.store.save_team_spec(&ctl.session_id.clone(), &spec).map_err(|e| e.to_string())?;
                for a in &spec.agents {
                    ctl.store.ensure_agent(&ctl.session_id.clone(), &a.id).map_err(|e| e.to_string())?;
                }
                Ok(json!({"revision": rev}))
            }),
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
            "begin_run" => self.with(id, &params, |ctl, p| {
                let run_id = p.get("run_id").and_then(|v| v.as_str()).ok_or("run_id required")?;
                ctl.begin_run(run_id).map(|run| json!({"run": run, "wake": ctl.wake_info(&run)}))
            }),
            "finalize_run" => self.with(id, &params, |ctl, p| {
                let run_id = p.get("run_id").and_then(|v| v.as_str()).ok_or("run_id required")?;
                let status: TurnStatus = serde_json::from_value(p.get("status").cloned().unwrap_or(Json::Null))
                    .map_err(|e| format!("bad status: {e}"))?;
                let outcome = TurnOutcome {
                    status,
                    error: p.get("error").and_then(|v| v.as_str()).map(str::to_string),
                    note: p.get("note").and_then(|v| v.as_str()).map(str::to_string),
                    reply_text: p.get("reply_text").and_then(|v| v.as_str()).map(str::to_string),
                };
                let ack_ids: Vec<i64> = p.get("ack_ids").and_then(|v| v.as_array()).map(|a| {
                    a.iter().filter_map(|v| v.as_i64()).collect()
                }).unwrap_or_default();
                ctl.finalize_run(run_id, &outcome, &ack_ids).map(|_| json!({"ok": true}))
            }),
            "emit" => self.with(id, &params, |ctl, p| {
                let drafts: Vec<EventDraft> = p
                    .get("events")
                    .and_then(|v| v.as_array())
                    .unwrap_or(&vec![])
                    .iter()
                    .filter_map(|d| {
                        let kind: EventKind = serde_json::from_value(d.get("kind").cloned()?).ok()?;
                        let mut draft = EventDraft::new(kind, d.get("payload").cloned().unwrap_or(json!({})));
                        draft.task_id = d.get("task_id").and_then(|v| v.as_str()).map(str::to_string);
                        draft.targets = d.get("targets").and_then(|v| serde_json::from_value(v.clone()).ok());
                        draft.actor_id = d.get("actor_id").and_then(|v| v.as_str()).map(str::to_string);
                        Some(draft)
                    })
                    .collect();
                let actor = p.get("actor_id").and_then(|v| v.as_str()).unwrap_or("system").to_string();
                ctl.emit(drafts, &actor);
                Ok(json!({"ok": true}))
            }),
            "schedule" => self.with(id, &params, |ctl, _p| {
                ctl.schedule();
                Ok(json!({"ok": true}))
            }),
            "agent_view" => self.with(id, &params, |ctl, p| {
                let agent = p.get("agent_id").and_then(|v| v.as_str()).ok_or("agent_id required")?;
                let spec = ctl.store.load_team_spec(&ctl.session_id.clone(), None).map_err(|e| e.to_string())?;
                Ok(teamagents_core::views::build_agent_view(&ctl.store, &spec, &ctl.session_id.clone(), agent))
            }),
            "state" => self.with(id, &params, |ctl, p| {
                let sid = ctl.session_id.clone();
                let after = p.get("after_sequence").and_then(|v| v.as_i64()).unwrap_or(0);
                let spec = ctl.store.load_team_spec(&sid, None).map_err(|e| e.to_string())?;
                let agents: Vec<Json> = spec
                    .agents
                    .iter()
                    .map(|a| {
                        json!({"id": a.id, "status": ctl.store.agent_status(&sid, &a.id).ok().flatten()})
                    })
                    .collect();
                Ok(json!({
                    "session": ctl.store.get_session(&sid).map_err(|e| e.to_string())?,
                    "spec": spec,
                    "leader_id": spec.leader_id,
                    "limits": spec.limits,
                    "revision": ctl.store.current_revision(&sid).map_err(|e| e.to_string())?,
                    "agents": agents,
                    "runs": ctl.store.runs_for_session(&sid, &[]).map_err(|e| e.to_string())?,
                    "tasks": ctl.store.tasks_for_session(&sid, &[]).map_err(|e| e.to_string())?,
                    "pending_approvals": ctl.store.pending_approvals(&sid).map_err(|e| e.to_string())?,
                    "events": ctl.store.events(&sid, after, 1000).map_err(|e| e.to_string())?,
                }))
            }),
            "shared_entries" => self.with(id, &params, |ctl, p| {
                let space_ids: Vec<String> = p.get("space_ids").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
                let after = p.get("after_sequence").and_then(|v| v.as_i64()).unwrap_or(0);
                let limit = p.get("limit").and_then(|v| v.as_i64()).unwrap_or(200);
                let entries = ctl.store.shared_entries(&ctl.session_id.clone(), &space_ids, after, limit).map_err(|e| e.to_string())?;
                Ok(json!({"entries": entries}))
            }),
            "approval_find_session" => self.with(id, &params, |ctl, p| {
                let hash = p.get("operation_hash").and_then(|v| v.as_str()).ok_or("operation_hash required")?;
                let scope = ctl.store.find_session_approval(&ctl.session_id.clone(), hash).map_err(|e| e.to_string())?;
                Ok(json!({"scope": scope}))
            }),
            "approval_for_call" => self.with(id, &params, |ctl, p| {
                let run_id = p.get("run_id").and_then(|v| v.as_str()).ok_or("run_id required")?;
                let call_id = p.get("tool_call_id").and_then(|v| v.as_str()).ok_or("tool_call_id required")?;
                let hash = p.get("operation_hash").and_then(|v| v.as_str()).ok_or("operation_hash required")?;
                Ok(json!({"approval": ctl.store.approval_for_call(run_id, call_id, hash).map_err(|e| e.to_string())?}))
            }),
            "insert_approval" => self.with(id, &params, |ctl, p| {
                let a: ApprovalRequest = serde_json::from_value(p.get("approval").cloned().unwrap_or(Json::Null))
                    .map_err(|e| format!("bad approval: {e}"))?;
                ctl.store.insert_approval(&a).map_err(|e| e.to_string())?;
                Ok(json!({"ok": true}))
            }),
            "drain_mid_turn" => self.with(id, &params, |ctl, _p| {
                let pushes: Vec<Json> = ctl
                    .drain_mid_turn_pushes()
                    .into_iter()
                    .map(|(run_id, items)| json!({"run_id": run_id, "items": items}))
                    .collect();
                Ok(json!({"pushes": pushes}))
            }),
            "validate_spec" => {
                // catalog-aware validation (cli.py::validate_spec) without a session
                let spec: Result<TeamSpec, _> = serde_json::from_value(params.get("spec").cloned().unwrap_or(Json::Null));
                match spec {
                    Err(e) => json!({"id": id, "error": format!("bad spec: {e}")}),
                    Ok(spec) => {
                        if let Err(e) = spec.validate() {
                            return json!({"id": id, "error": e});
                        }
                        let models: Vec<String> = params.get("models").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
                        let tools: Vec<String> = params.get("tools").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
                        let unknown_models: Vec<&str> = spec.agents.iter().map(|a| a.model_profile.as_str()).filter(|m| !models.contains(&m.to_string())).collect();
                        let unknown_tools: Vec<&str> = spec.agents.iter().flat_map(|a| a.tool_bindings.iter().map(|t| t.as_str())).filter(|t| !tools.contains(&t.to_string()) && !teamagents_core::control::BUILTIN_TOOL_BINDINGS.contains(t)).collect();
                        if !unknown_models.is_empty() || !unknown_tools.is_empty() {
                            return json!({"id": id, "error": format!("unknown model profiles {unknown_models:?}, unknown tool bindings {unknown_tools:?}")});
                        }
                        json!({"id": id, "result": {"leader": spec.leader_id, "members": spec.agents.len(),
                                                     "channels": spec.channels.len(), "spaces": spec.shared_spaces.len()}})
                    }
                }
            }
            "get_approval" => self.with(id, &params, |ctl, p| {
                let aid = p.get("approval_id").and_then(|v| v.as_str()).ok_or("approval_id required")?;
                Ok(json!({"approval": ctl.store.get_approval(aid).map_err(|e| e.to_string())?}))
            }),
            "requeue_run" => self.with(id, &params, |ctl, p| {
                let run_id = p.get("run_id").and_then(|v| v.as_str()).ok_or("run_id required")?;
                let ok = ctl.store.update_run_status_where(run_id, TurnStatus::Running, TurnStatus::Queued).map_err(|e| e.to_string())?;
                Ok(json!({"requeued": ok}))
            }),
            "set_run_status" => self.with(id, &params, |ctl, p| {
                let run_id = p.get("run_id").and_then(|v| v.as_str()).ok_or("run_id required")?;
                let status: TurnStatus = serde_json::from_value(p.get("status").cloned().unwrap_or(Json::Null)).map_err(|e| e.to_string())?;
                ctl.store.set_run_status(run_id, status).map_err(|e| e.to_string())?;
                if status == TurnStatus::WaitingApproval {
                    if let Ok(Some(run)) = ctl.store.get_run(run_id) {
                        let _ = ctl.store.set_agent_status(&ctl.session_id.clone(), &run.agent_id, AgentStatus::Waiting);
                    }
                } else if status == TurnStatus::Running {
                    if let Ok(Some(run)) = ctl.store.get_run(run_id) {
                        let _ = ctl.store.set_agent_status(&ctl.session_id.clone(), &run.agent_id, AgentStatus::Busy);
                    }
                }
                Ok(json!({"ok": true}))
            }),
            "set_run_external_turn" => self.with(id, &params, |ctl, p| {
                let run_id = p.get("run_id").and_then(|v| v.as_str()).ok_or("run_id required")?;
                let ext = p.get("external_turn_id").and_then(|v| v.as_str()).ok_or("external_turn_id required")?;
                ctl.store.set_run_external_turn(run_id, ext).map_err(|e| e.to_string())?;
                Ok(json!({"ok": true}))
            }),
            "get_codex_thread" => self.with(id, &params, |ctl, p| {
                let agent = p.get("agent_id").and_then(|v| v.as_str()).ok_or("agent_id required")?;
                Ok(json!({"thread_id": ctl.store.get_codex_thread(&ctl.session_id.clone(), agent).map_err(|e| e.to_string())?}))
            }),
            "set_codex_thread" => self.with(id, &params, |ctl, p| {
                let agent = p.get("agent_id").and_then(|v| v.as_str()).ok_or("agent_id required")?;
                let tid = p.get("thread_id").and_then(|v| v.as_str()).ok_or("thread_id required")?;
                ctl.store.set_codex_thread(&ctl.session_id.clone(), agent, tid).map_err(|e| e.to_string())?;
                Ok(json!({"ok": true}))
            }),
            "stop_timeout" => self.with(id, &params, |ctl, p| {
                let run_id = p.get("run_id").and_then(|v| v.as_str()).ok_or("run_id required")?;
                ctl.stop_timeout(run_id)?;
                Ok(json!({"ok": true}))
            }),
            _ => json!({"id": id, "error": format!("unknown method {method:?}")}),
        }
    }

    /// Run f against the session's Control; f returns Err(String) on failure.
    fn with(&mut self, id: Json, params: &Json, f: impl FnOnce(&mut Control, &Json) -> Result<Json, String>) -> Json {
        let Some(sid) = params.get("session_id").and_then(|v| v.as_str()).map(str::to_string) else {
            return json!({"id": id, "error": "session_id required"});
        };
        match self.control_for(&sid) {
            Ok(ctl) => match f(ctl, params) {
                Ok(result) => json!({"id": id, "result": result}),
                Err(e) => json!({"id": id, "error": e}),
            },
            Err(e) => json!({"id": id, "error": e}),
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
