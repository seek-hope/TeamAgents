//! Headless session worker for the TUI (the former tui-worker.ts protocol).
//! One JSON request per line on stdin:  {"id": N, "method": "...", "params": {...}}
//! One JSON response per line:          {"id": N, "result": ...} | {"id": N, "error": "..."}
//! Async pushes (no id):                {"push": "delta", "run_id", "agent_id", "text"}
//!
//! The worker owns the Runtime; the Rust TUI is a pure client of this protocol.

use crate::scripted::Step;
use crate::session::{open_session, OpenOptions, OpenedSession};
use crate::sessions::{archive_session, delete_session, list_sessions, new_session_id, session_paths};
use crate::{config, VERSION};
use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Per-member working plans (`members/<id>/plan.json`), for the UI status strip.
fn member_plans(session_id: &str) -> Json {
    let members = crate::sessions::session_paths(session_id).base.join("members");
    let mut out: Vec<Json> = vec![];
    if let Ok(entries) = std::fs::read_dir(&members) {
        for entry in entries.flatten() {
            let Ok(bytes) = std::fs::read(entry.path().join("plan.json")) else { continue };
            let Ok(value) = serde_json::from_slice::<Json>(&bytes) else { continue };
            out.push(json!({
                "agent_id": entry.file_name().to_string_lossy(),
                "items": value.get("items").cloned().unwrap_or(json!([])),
                "updated_ms": value.get("updated_ms").cloned().unwrap_or(Json::Null),
            }));
        }
    }
    Json::Array(out)
}

fn out(message: &Json) {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let _ = writeln!(handle, "{message}");
    let _ = handle.flush();
}

struct Worker {
    opened: Mutex<Option<Arc<OpenedSession>>>,
    cwd: Mutex<Option<PathBuf>>,
    full_auto: Mutex<bool>,
    team: Mutex<Option<String>>,
}

impl Worker {
    fn new() -> Self {
        Self { opened: Mutex::new(None), cwd: Mutex::new(None), full_auto: Mutex::new(false), team: Mutex::new(None) }
    }

    fn current(&self) -> Result<Arc<OpenedSession>, String> {
        self.opened.lock().unwrap().clone().ok_or_else(|| "no open session".to_string())
    }

    fn close_current(&self) {
        let current = self.opened.lock().unwrap().take();
        if let Some(current) = current {
            current.close();
        }
    }

    fn open(&self, params: &Json) -> Result<Json, String> {
        let cwd =
            params.get("cwd").and_then(|v| v.as_str()).map(PathBuf::from).or_else(|| std::env::current_dir().ok());
        let full_auto = params.get("fullAuto").and_then(|v| v.as_bool()).unwrap_or(false);
        let team = params.get("team").and_then(|v| v.as_str()).map(str::to_string);

        let initial_spec = match &team {
            Some(path) => Some(config::load_spec_file(std::path::Path::new(path))?),
            // fork passes the source session's live spec directly (D-26)
            None => params.get("initial_spec").cloned(),
        };
        let scripts: Option<HashMap<String, Vec<Step>>> = params
            .get("scripts")
            .and_then(|v| v.as_object())
            .map(|map| {
                map.iter()
                    .map(|(agent, steps)| {
                        let parsed = steps
                            .as_array()
                            .map(|array| array.iter().map(Step::from_json).collect::<Result<Vec<_>, _>>())
                            .unwrap_or(Ok(vec![]));
                        Ok((agent.clone(), parsed?))
                    })
                    .collect::<Result<HashMap<_, _>, String>>()
            })
            .transpose()?;

        let opened = open_session(OpenOptions {
            cwd,
            session_id: params.get("resume").and_then(|v| v.as_str()).map(str::to_string),
            full_auto,
            initial_spec,
            catalog: None,
            scripts,
        })?;
        let sink_session = opened.session_id.clone();
        opened.runtime.notify.set_stream_sink(Box::new(move |run_id, agent_id, text| {
            out(&json!({"push": "delta", "session_id": sink_session, "run_id": run_id, "agent_id": agent_id, "text": text}));
        }));
        let plan_session = opened.session_id.clone();
        opened.runtime.notify.set_plan_sink(Box::new(move |agent_id, items| {
            out(&json!({"push": "plan", "session_id": plan_session, "agent_id": agent_id, "items": items}));
        }));
        let tool_session = opened.session_id.clone();
        opened.runtime.notify.set_tool_sink(Box::new(move |run_id, agent_id, activity| {
            out(&json!({"push": "tool", "session_id": tool_session, "run_id": run_id, "agent_id": agent_id,
                        "tool": activity["tool"], "ok": activity["ok"], "arguments": activity["arguments"],
                        "result": activity["result"]}));
        }));
        opened.runtime.start();
        // Stage the replacement completely before touching the current one.
        // A bad resume/config must leave the active session usable.
        let previous = self.opened.lock().unwrap().replace(opened.clone());
        *self.cwd.lock().unwrap() = Some(opened.cwd.clone());
        *self.full_auto.lock().unwrap() = full_auto;
        *self.team.lock().unwrap() = team.clone();
        if let Some(previous) = previous {
            previous.close();
        }
        Ok(json!({
            "session_id": opened.session_id,
            "state_dir": config::state_dir().to_string_lossy(),
            "sessions_dir": config::sessions_dir().to_string_lossy(),
            "user_config_path": config::user_config_path().to_string_lossy(),
            "catalog": opened.catalog(),
        }))
    }

    fn handle(&self, method: &str, params: &Json) -> Result<Json, String> {
        match method {
            "ping" => Ok(json!({"worker": VERSION})),
            "open" => self.open(params),
            "call" => {
                let opened = self.current()?;
                let inner = params.get("method").and_then(|v| v.as_str()).ok_or("method required")?;
                let mut reply =
                    opened.runtime.core.call_in_session(inner, params.get("params").cloned().unwrap_or(json!({})))?;
                // member plans live in the member directories; the UI reads them
                // from the same snapshot it already polls
                if inner == "state" {
                    if let Some(object) = reply.as_object_mut() {
                        object.insert("plans".into(), member_plans(&opened.session_id));
                        // per-member context usage rides along: the team panel shows
                        // how close each member is to compaction
                        object.insert(
                            "usage".into(),
                            opened.usage_report().get("agents").cloned().unwrap_or_else(|| json!([])),
                        );
                    }
                }
                Ok(reply)
            }
            "submit" => {
                let opened = self.current()?;
                let mut action = params.get("action").cloned().unwrap_or(json!({}));
                let Some(object) = action.as_object_mut() else { return Err("action must be an object".into()) };
                object.insert("session_id".into(), json!(opened.session_id));
                object.entry("actor_id").or_insert(json!("user"));
                let action: teamagents_core::models::TeamAction =
                    serde_json::from_value(action).map_err(|e| format!("bad action: {e}"))?;
                let receipt = opened.runtime.submit(action)?;
                Ok(serde_json::to_value(receipt).map_err(|e| e.to_string())?)
            }
            "user_message" => {
                let opened = self.current()?;
                let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let supplement = params.get("supplement").and_then(|v| v.as_bool()).unwrap_or(false);
                let receipt = opened.runtime.user_message(&text, supplement)?;
                Ok(serde_json::to_value(receipt).map_err(|e| e.to_string())?)
            }
            "list_sessions" => {
                let cwd = params
                    .get("cwd")
                    .and_then(|v| v.as_str())
                    .map(PathBuf::from)
                    .or_else(|| self.cwd.lock().unwrap().clone());
                Ok(json!({"sessions": list_sessions(cwd.as_deref(), true, None)}))
            }
            "switch_session" => {
                let session_id = params.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                self.open(&json!({
                    "cwd": self.cwd.lock().unwrap().clone().map(|p| p.to_string_lossy().into_owned()),
                    "resume": session_id,
                    "fullAuto": *self.full_auto.lock().unwrap(),
                    "team": self.team.lock().unwrap().clone(),
                }))
            }
            "new_session" => {
                let cwd = self.cwd.lock().unwrap().clone().unwrap_or_else(|| PathBuf::from("."));
                self.open(&json!({
                    "cwd": self.cwd.lock().unwrap().clone().map(|p| p.to_string_lossy().into_owned()),
                    "resume": new_session_id(&cwd),
                    "fullAuto": *self.full_auto.lock().unwrap(),
                    "team": self.team.lock().unwrap().clone(),
                }))
            }
            "archive_session" => {
                let session_id = params.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let was_current =
                    self.opened.lock().unwrap().as_ref().map(|o| o.session_id == session_id).unwrap_or(false);
                if was_current {
                    self.close_current();
                }
                let target = archive_session(&session_id, None)?;
                Ok(json!({"was_current": was_current, "target": target}))
            }
            "delete_session" => {
                let session_id = params.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let was_current =
                    self.opened.lock().unwrap().as_ref().map(|o| o.session_id == session_id).unwrap_or(false);
                if was_current {
                    self.close_current();
                }
                delete_session(&session_id, None)?;
                Ok(json!({"was_current": was_current}))
            }
            "usage" => Ok(self.current()?.usage_report()),
            // feature 5 (/model): effective model/effort per member
            "model" => Ok(self.current()?.model_report()),
            "set_model" => {
                let opened = self.current()?;
                let agent_id = params.get("agent_id").and_then(|v| v.as_str()).ok_or("agent_id required")?;
                let model = params.get("model").and_then(|v| v.as_str()).map(str::to_string);
                let effort = params.get("effort").and_then(|v| v.as_str()).map(str::to_string);
                let profile = params.get("profile").and_then(|v| v.as_str()).map(str::to_string);
                if profile.is_some() {
                    opened.set_model_selection(agent_id, profile, model, effort)
                } else {
                    opened.set_model_override(agent_id, model, effort)
                }
            }
            // D-26 rewind/fork (pi-style tree history, leader conversation)
            "rewind_points" => Ok(self.current()?.rewind_points()?),
            "rewind" => {
                let node = params.get("node_id").and_then(|v| v.as_str()).map(str::to_string);
                Ok(self.current()?.rewind(node)?)
            }
            "fork_session" => {
                let opened = self.current()?;
                let state = opened.core.call_in_session("state", json!({"include_events": false}))?;
                // fork while a turn is in flight would cancel it on close and
                // BLOCK its task (see AGENTS.md operations notes)
                let any_active =
                    state.get("runs").and_then(|v| v.as_array()).cloned().unwrap_or_default().into_iter().any(|r| {
                        r.get("status")
                            .and_then(|v| v.as_str())
                            .map(|s| s == "QUEUED" || s == "RUNNING")
                            .unwrap_or(false)
                    });
                if any_active {
                    return Err("有回合进行中，等它结束后再 fork".into());
                }
                let leader = state.get("leader_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let source_epoch = state
                    .get("agents")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.iter().find(|x| x.get("id").and_then(|v| v.as_str()) == Some(leader.as_str())))
                    .and_then(|a| a.get("context_epoch"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(1);
                let spec = state.get("spec").cloned().ok_or("no spec")?;
                let cwd = self.cwd.lock().unwrap().clone().unwrap_or_else(|| PathBuf::from("."));
                // fork = same spec + leader's history tree, fresh team state
                // (tasks/runs/events are facts of the source session and are
                // NOT copied; file changes are never rolled back — D-26)
                let new_id = new_session_id(&cwd);
                let source_member = session_paths(&opened.session_id).base.join("members").join(&leader);
                let staged_member = session_paths(&new_id).base.join("members").join(&leader);
                std::fs::create_dir_all(&staged_member).map_err(|e| e.to_string())?;
                // Configuration and conversation files are the only fork inputs.
                // Team DB facts, usage, and runs stay fresh in the destination.
                for name in ["profiles.json", "model_overrides.json"] {
                    let src = session_paths(&opened.session_id).base.join(name);
                    if src.is_file() {
                        std::fs::copy(&src, session_paths(&new_id).base.join(name)).map_err(|e| e.to_string())?;
                    }
                }
                for name in ["chat_tree.json", "chat_history.json"] {
                    let src = source_member.join(name);
                    if src.is_file() {
                        std::fs::copy(&src, staged_member.join(name)).map_err(|e| e.to_string())?;
                    }
                }
                let mut out = match self.open(&json!({
                    "cwd": cwd.to_string_lossy(),
                    "resume": new_id,
                    "fullAuto": *self.full_auto.lock().unwrap(),
                    "team": Json::Null,
                    "initial_spec": spec,
                })) {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = std::fs::remove_dir_all(&session_paths(&new_id).base);
                        return Err(error);
                    }
                };
                // The new DB starts a fresh context epoch. Remap the copied
                // leader tree key to that epoch while retaining all branches.
                if let Some(new_opened) = self.opened.lock().unwrap().clone() {
                    let state = new_opened.core.call_in_session("state", json!({"include_events": false}))?;
                    let epoch = state
                        .get("agents")
                        .and_then(|v| v.as_array())
                        .and_then(|a| a.iter().find(|x| x.get("id").and_then(|v| v.as_str()) == Some(leader.as_str())))
                        .and_then(|a| a.get("context_epoch"))
                        .and_then(|v| v.as_i64())
                        .unwrap_or(1);
                    for name in ["chat_tree.json", "chat_history.json"] {
                        let path = session_paths(&new_id).base.join("members").join(&leader).join(name);
                        if let Ok(text) = std::fs::read_to_string(&path) {
                            if let Ok(mut tree) = serde_json::from_str::<Json>(&text) {
                                if let Some(obj) = tree.as_object_mut() {
                                    let source_key = format!("ctx:{leader}:{source_epoch}");
                                    if let Some(value) = obj.remove(&source_key) {
                                        obj.insert(format!("ctx:{leader}:{epoch}"), value);
                                        std::fs::write(
                                            &path,
                                            serde_json::to_vec_pretty(&tree).map_err(|e| e.to_string())?,
                                        )
                                        .map_err(|e| e.to_string())?;
                                    }
                                }
                            }
                        }
                    }
                }
                out["forked_from"] = json!(opened.session_id.clone());
                Ok(out)
            }
            "close" => {
                self.close_current();
                Ok(json!({"ok": true}))
            }
            other => Err(format!("unknown method {other}")),
        }
    }
}

pub fn serve() -> i32 {
    let worker = Worker::new();
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<Json>(&line) else { continue };
        let id = request.get("id").cloned().unwrap_or(Json::Null);
        let method = request.get("method").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let params = request.get("params").cloned().unwrap_or(json!({}));
        // Slow read-only discovery must not block cancellation, polling or close.
        if method == "discover_models" {
            match (worker.current(), params["provider"].as_str().map(str::to_string)) {
                (Ok(opened), Some(provider)) => {
                    std::thread::spawn(move || match opened.discover_models(&provider) {
                        Ok(result) => out(&json!({"id":id, "result":result})),
                        Err(error) => out(&json!({"id":id, "error":error})),
                    });
                }
                (Err(error), _) => out(&json!({"id":id, "error":error})),
                (_, None) => out(&json!({"id":id, "error":"provider required"})),
            }
            continue;
        }
        match worker.handle(&method, &params) {
            Ok(result) => out(&json!({"id": id, "result": result})),
            Err(e) => out(&json!({"id": id, "error": e})),
        }
        if method == "close" {
            break;
        }
    }
    worker.close_current();
    0
}
