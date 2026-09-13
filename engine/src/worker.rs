//! Headless session worker for the TUI (the former tui-worker.ts protocol).
//! One JSON request per line on stdin:  {"id": N, "method": "...", "params": {...}}
//! One JSON response per line:          {"id": N, "result": ...} | {"id": N, "error": "..."}
//! Async pushes (no id):                {"push": "delta", "run_id", "agent_id", "text"}
//!
//! The worker owns the Runtime; the Rust TUI is a pure client of this protocol.

use crate::session::{open_session, OpenOptions, OpenedSession};
use crate::sessions::{archive_session, delete_session, list_sessions, new_session_id};
use crate::scripted::Step;
use crate::{config, VERSION};
use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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
        Self {
            opened: Mutex::new(None),
            cwd: Mutex::new(None),
            full_auto: Mutex::new(false),
            team: Mutex::new(None),
        }
    }

    fn current(&self) -> Result<Arc<OpenedSession>, String> {
        self.opened
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "no open session".to_string())
    }

    fn close_current(&self) {
        let current = self.opened.lock().unwrap().take();
        if let Some(current) = current {
            current.close();
        }
    }

    fn open(&self, params: &Json) -> Result<Json, String> {
        self.close_current();
        let cwd = params
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok());
        let full_auto = params.get("fullAuto").and_then(|v| v.as_bool()).unwrap_or(false);
        let team = params.get("team").and_then(|v| v.as_str()).map(str::to_string);
        *self.cwd.lock().unwrap() = cwd.clone();
        *self.full_auto.lock().unwrap() = full_auto;
        *self.team.lock().unwrap() = team.clone();

        let initial_spec = match &team {
            Some(path) => {
                let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read team spec {path}: {e}"))?;
                Some(serde_json::from_str::<Json>(&text).map_err(|e| format!("bad team spec {path}: {e}"))?)
            }
            None => None,
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
            let _ = sink_session;
            out(&json!({"push": "delta", "run_id": run_id, "agent_id": agent_id, "text": text}));
        }));
        opened.runtime.start();
        *self.opened.lock().unwrap() = Some(opened.clone());
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
                opened.runtime.core.call_in_session(inner, params.get("params").cloned().unwrap_or(json!({})))
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
                let was_current = self.opened.lock().unwrap().as_ref().map(|o| o.session_id == session_id).unwrap_or(false);
                if was_current {
                    self.close_current();
                }
                let target = archive_session(&session_id, None)?;
                Ok(json!({"was_current": was_current, "target": target}))
            }
            "delete_session" => {
                let session_id = params.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let was_current = self.opened.lock().unwrap().as_ref().map(|o| o.session_id == session_id).unwrap_or(false);
                if was_current {
                    self.close_current();
                }
                delete_session(&session_id, None)?;
                Ok(json!({"was_current": was_current}))
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
