//! Deterministic scripted member used by the scenario
//! tests and by the worker's `scripts` option.
//!
//! Steps: ["call", tool, args] | ["barrier", name] | ["sleep", seconds]
//!        ["wait"] | ["end"] | ["fail", msg] | ["inbox"]

use crate::gateway::ToolGateway;
use crate::runtime::AgentRunner;
use serde_json::{json, Value as Json};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use teamagents_core::control::TurnOutcome;
use teamagents_core::models::{TurnRun, TurnStatus};

#[derive(Debug, Clone)]
pub enum Step {
    Call(String, Json),
    Barrier(String),
    Sleep(f64),
    Wait,
    End,
    Fail(String),
    Inbox,
}

impl Step {
    pub fn from_json(value: &Json) -> Result<Step, String> {
        let array = value.as_array().ok_or_else(|| format!("bad step {value}"))?;
        let kind = array.first().and_then(|v| v.as_str()).unwrap_or("");
        Ok(match kind {
            "call" => Step::Call(
                array.get(1).and_then(|v| v.as_str()).unwrap_or("").to_string(),
                array.get(2).cloned().unwrap_or(json!({})),
            ),
            "barrier" => Step::Barrier(array.get(1).and_then(|v| v.as_str()).unwrap_or("").to_string()),
            "sleep" => Step::Sleep(array.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0)),
            "wait" => Step::Wait,
            "end" => Step::End,
            "fail" => Step::Fail(array.get(1).and_then(|v| v.as_str()).unwrap_or("failed").to_string()),
            "inbox" => Step::Inbox,
            other => return Err(format!("unknown step {other:?}")),
        })
    }
}

/// One-shot barrier: the first `parties` arrivals pass, later waits return at once.
pub struct Barrier {
    parties: usize,
    arrived: AtomicUsize,
}

impl Barrier {
    pub fn new(parties: usize) -> Arc<Self> {
        Arc::new(Self { parties, arrived: AtomicUsize::new(0) })
    }
    pub fn arrive(&self) -> usize {
        self.arrived.fetch_add(1, Ordering::SeqCst) + 1
    }
    pub fn passed(&self) -> bool {
        self.arrived.load(Ordering::SeqCst) >= self.parties
    }
}

pub type BarrierRegistry = Arc<Mutex<HashMap<String, Arc<Barrier>>>>;

/// Templates in scripts: "$r0.result.task_id", "$inbox0.payload.task_id",
/// "$run.task_id".
fn resolve_refs(value: &Json, ctx: &HashMap<String, Json>) -> Json {
    match value {
        Json::String(s) if s.starts_with('$') => {
            let mut parts = s[1..].split('.');
            let head = parts.next().unwrap_or("");
            let mut current = ctx.get(head).cloned().unwrap_or(Json::Null);
            for part in parts {
                if current.is_null() {
                    break;
                }
                current = match &current {
                    Json::Array(items) if part.chars().all(|c| c.is_ascii_digit()) => {
                        let index: usize = part.parse().unwrap_or(usize::MAX);
                        items.get(index).cloned().unwrap_or(Json::Null)
                    }
                    other => other.get(part).cloned().unwrap_or(Json::Null),
                };
            }
            current
        }
        Json::Array(items) => Json::Array(items.iter().map(|v| resolve_refs(v, ctx)).collect()),
        Json::Object(map) => Json::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), resolve_refs(v, ctx)))
                .collect(),
        ),
        other => other.clone(),
    }
}

pub struct ScriptedMember {
    pub agent_id: String,
    script: Mutex<Vec<Step>>,
    pub cursor: AtomicUsize,
    pub results: Mutex<Vec<Json>>,
    pub observed_inbox: Mutex<Vec<Json>>,
    states: Mutex<HashMap<String, TurnStatus>>,
    mid_turn: Mutex<HashMap<String, Vec<Json>>>,
    cancelled: Mutex<HashSet<String>>,
    barriers: BarrierRegistry,
    pub last_view: Mutex<Option<Json>>,
}

impl ScriptedMember {
    pub fn new(agent_id: &str, script: Vec<Step>, barriers: BarrierRegistry) -> Arc<Self> {
        Arc::new(Self {
            agent_id: agent_id.to_string(),
            script: Mutex::new(script),
            cursor: AtomicUsize::new(0),
            results: Mutex::new(vec![]),
            observed_inbox: Mutex::new(vec![]),
            states: Mutex::new(HashMap::new()),
            mid_turn: Mutex::new(HashMap::new()),
            cancelled: Mutex::new(HashSet::new()),
            barriers,
            last_view: Mutex::new(None),
        })
    }

    /// Tests re-arm the same member for a second turn (t9).
    pub fn reset(&self, script: Vec<Step>) {
        *self.script.lock().unwrap() = script;
        self.cursor.store(0, Ordering::SeqCst);
    }

    pub fn remaining(&self) -> usize {
        let script = self.script.lock().unwrap();
        script.len().saturating_sub(self.cursor.load(Ordering::SeqCst))
    }

    pub fn is_cancelled(&self, run_id: &str) -> bool {
        self.cancelled.lock().unwrap().contains(run_id)
    }

    fn sleep_or_cancel(&self, run_id: &str, ms: u64) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        while std::time::Instant::now() < deadline {
            if self.is_cancelled(run_id) {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        true
    }
}

impl AgentRunner for ScriptedMember {
    fn start_or_resume(&self, run: &TurnRun, view: &Json, gateway: &ToolGateway, wake: &Json) -> TurnOutcome {
        self.states.lock().unwrap().insert(run.run_id.clone(), TurnStatus::Running);
        *self.last_view.lock().unwrap() = Some(view.clone());
        let mut ctx: HashMap<String, Json> = HashMap::new();
        ctx.insert(
            "run".into(),
            json!({
                "task_id": run.task_id,
                "run_id": run.run_id,
                "agent_id": run.agent_id,
                "wake": wake.get("reason").cloned().unwrap_or(Json::Null),
            }),
        );
        let inbox = view.get("inbox_delta").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        for (i, item) in inbox.iter().enumerate() {
            ctx.insert(format!("inbox{i}"), item.clone());
        }
        for (i, result) in self.results.lock().unwrap().iter().enumerate() {
            ctx.insert(format!("r{i}"), result.clone());
        }

        loop {
            let index = self.cursor.load(Ordering::SeqCst);
            let step = self.script.lock().unwrap().get(index).cloned();
            let Some(step) = step else {
                self.states.lock().unwrap().insert(run.run_id.clone(), TurnStatus::Completed);
                return TurnOutcome {
                    status: TurnStatus::Completed,
                    error: None,
                    note: None,
                    reply_text: None,
                };
            };
            match step {
                Step::Call(tool, args) => {
                    let resolved = resolve_refs(&args, &ctx);
                    let receipt = gateway.call(&tool, &resolved, &format!("step{index}"));
                    let value = serde_json::to_value(&receipt).unwrap_or(Json::Null);
                    let count = {
                        let mut results = self.results.lock().unwrap();
                        results.push(value.clone());
                        results.len()
                    };
                    ctx.insert(format!("r{}", count - 1), value.clone());
                    self.cursor.store(index + 1, Ordering::SeqCst);
                    if receipt.error.as_deref() == Some("approval_required") {
                        self.states.lock().unwrap().insert(run.run_id.clone(), TurnStatus::WaitingApproval);
                        let note = receipt
                            .result
                            .get("approval_id")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        return TurnOutcome {
                            status: TurnStatus::WaitingApproval,
                            error: None,
                            note,
                            reply_text: None,
                        };
                    }
                }
                Step::Barrier(name) => {
                    let barrier = {
                        let mut registry = self.barriers.lock().unwrap();
                        registry
                            .entry(name)
                            .or_insert_with(|| Barrier::new(2))
                            .clone()
                    };
                    barrier.arrive();
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
                    while !barrier.passed() {
                        if self.is_cancelled(&run.run_id) {
                            break;
                        }
                        if std::time::Instant::now() > deadline {
                            self.states.lock().unwrap().insert(run.run_id.clone(), TurnStatus::Failed);
                            return TurnOutcome {
                                status: TurnStatus::Failed,
                                error: Some("step timed out after 15000ms".into()),
                                note: None,
                                reply_text: None,
                            };
                        }
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    self.cursor.store(index + 1, Ordering::SeqCst);
                }
                Step::Sleep(seconds) => {
                    let ms = (seconds * 1000.0) as u64;
                    if !self.sleep_or_cancel(&run.run_id, ms) {
                        self.states.lock().unwrap().insert(run.run_id.clone(), TurnStatus::Cancelled);
                        return TurnOutcome {
                            status: TurnStatus::Cancelled,
                            error: None,
                            note: Some("interrupted".into()),
                            reply_text: None,
                        };
                    }
                    self.cursor.store(index + 1, Ordering::SeqCst);
                }
                Step::Inbox => {
                    let mut items = view.get("inbox_delta").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                    if let Some(more) = self.mid_turn.lock().unwrap().remove(&run.run_id) {
                        items.extend(more);
                    }
                    self.observed_inbox.lock().unwrap().extend(items.clone());
                    self.results.lock().unwrap().push(json!({
                        "action_id": format!("inbox:{index}"),
                        "ok": true,
                        "kind": "send_message",
                        "result": { "injected": items },
                        "error": null,
                    }));
                    self.cursor.store(index + 1, Ordering::SeqCst);
                }
                Step::Wait => {
                    self.cursor.store(index + 1, Ordering::SeqCst);
                    self.states.lock().unwrap().insert(run.run_id.clone(), TurnStatus::WaitingTask);
                    return TurnOutcome {
                        status: TurnStatus::WaitingTask,
                        error: None,
                        note: None,
                        reply_text: None,
                    };
                }
                Step::End => {
                    self.cursor.store(index + 1, Ordering::SeqCst);
                    self.states.lock().unwrap().insert(run.run_id.clone(), TurnStatus::Completed);
                    return TurnOutcome {
                        status: TurnStatus::Completed,
                        error: None,
                        note: None,
                        reply_text: None,
                    };
                }
                Step::Fail(message) => {
                    self.cursor.store(index + 1, Ordering::SeqCst);
                    self.states.lock().unwrap().insert(run.run_id.clone(), TurnStatus::Failed);
                    return TurnOutcome {
                        status: TurnStatus::Failed,
                        error: Some(message),
                        note: None,
                        reply_text: None,
                    };
                }
            }
            if self.is_cancelled(&run.run_id) {
                self.states.lock().unwrap().insert(run.run_id.clone(), TurnStatus::Cancelled);
                return TurnOutcome {
                    status: TurnStatus::Cancelled,
                    error: None,
                    note: Some("interrupted".into()),
                    reply_text: None,
                };
            }
        }
    }

    fn request_interrupt(&self, run_id: &str) -> TurnStatus {
        self.cancelled.lock().unwrap().insert(run_id.to_string());
        self.states.lock().unwrap().insert(run_id.to_string(), TurnStatus::Cancelled);
        TurnStatus::Cancelled
    }

    fn query_state(&self, run_id: &str) -> Option<TurnStatus> {
        self.states.lock().unwrap().get(run_id).copied()
    }

    fn deliver_mid_turn(&self, run_id: &str, items: Vec<Json>) {
        let mut map = self.mid_turn.lock().unwrap();
        map.entry(run_id.to_string()).or_default().extend(items);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_resolve_into_nested_payloads() {
        let mut ctx = HashMap::new();
        ctx.insert("run".into(), json!({"task_id": "t1"}));
        ctx.insert("r0".into(), json!({"result": {"task_id": "task_abc"}}));
        ctx.insert("inbox0".into(), json!({"payload": {"task_id": "task_inbox"}}));
        assert_eq!(resolve_refs(&json!("$r0.result.task_id"), &ctx), json!("task_abc"));
        assert_eq!(resolve_refs(&json!("$inbox0.payload.task_id"), &ctx), json!("task_inbox"));
        assert_eq!(resolve_refs(&json!("$run.task_id"), &ctx), json!("t1"));
        assert_eq!(
            resolve_refs(&json!({"task_ids": ["$run.task_id", "literal"]}), &ctx),
            json!({"task_ids": ["t1", "literal"]})
        );
        assert_eq!(resolve_refs(&json!("$r9.missing"), &ctx), Json::Null);
    }
}
