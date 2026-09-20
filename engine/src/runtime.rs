//! Session execution loop (thread-per-turn).
//! Every authoritative state change goes through the core; this module only
//! decides *when* a member turn runs, and reports its outcome.

use crate::core_client::CoreClient;
use crate::gateway::{ApprovalGate, ToolGateway, TurnControl};
use serde_json::{json, Value as Json};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};
use teamagents_core::control::{FinalizationResult, TurnOutcome};
use teamagents_core::models::{AgentSpec, AgentStatus, Receipt, TeamAction, TurnRun, TurnStatus};

/// Best-effort core call: the core now reports real write failures, so log them
/// loudly instead of dropping them (silent loss is how the emit/schedule bug
/// went unnoticed; review/findings-rust-review-2026-09-13.md S3).
fn core_best_effort(core: &CoreClient, what: &str, method: &str, params: Json) {
    if let Err(e) = core.call_in_session(method, params) {
        eprintln!("teamagents: {what} failed: {e}");
    }
}

pub trait AgentRunner: Send + Sync {
    fn start_or_resume(&self, run: &TurnRun, view: &Json, gateway: &ToolGateway, wake: &Json) -> TurnOutcome;
    fn request_interrupt(&self, run_id: &str) -> TurnStatus;
    fn query_state(&self, run_id: &str) -> Option<TurnStatus>;
    /// Durable input IDs, when a backend checkpoints its own delivery boundary.
    fn applied_delivery_ids(&self, _run: &TurnRun) -> Option<Vec<i64>> {
        None
    }
    fn deliver_mid_turn(&self, run_id: &str, items: Vec<Json>);
    /// Read-only evidence that a queued intent was already executed. Missing
    /// evidence leaves a genuinely new intent queued; unreadable evidence fails.
    fn has_recovery_state(&self, run: &TurnRun) -> Result<bool, String> {
        Ok(run.external_turn_id.is_some())
    }
    /// Optional restart convergence (RT-04). Return the recorded result as
    /// well as its status; the gateway may restore a team completion receipt.
    /// Reconciliation must not replay external execution tools.
    fn reconcile(&self, _run: &TurnRun, _gateway: &ToolGateway) -> Option<TurnOutcome> {
        None
    }
    /// Optional: resolve a parked approval in a backend that asked for it.
    fn resolve_approval(&self, _approval_id: &str, _decision: &str) -> bool {
        false
    }
    /// Backends holding in-process approval waiters (codex). A `false`
    /// resolve_approval only warrants a warning when a waiter could exist;
    /// requeue-driven backends (chat/scripted) take the false as normal.
    fn has_approval_waiter(&self) -> bool {
        false
    }
    /// D-26 rewind: chat backends own a tree-structured history; backends that
    /// keep history server-side (codex) report unsupported — use their native
    /// fork instead.
    fn rewind_points(&self, _thread: &str) -> Result<Vec<Json>, String> {
        Ok(vec![])
    }
    fn rewind(&self, _thread: &str, _node: Option<&str>) -> Result<usize, String> {
        Err("this backend does not support rewind".into())
    }
    fn close(&self) {}
}

pub type RunnerFactory = Box<dyn Fn(&AgentSpec) -> Result<Arc<dyn AgentRunner>, String> + Send + Sync>;
pub type ToolExecutor = Arc<dyn Fn(&str, &str, &Json, &TurnControl) -> Result<Json, String> + Send + Sync>;

#[derive(Debug, Clone)]
pub struct RuntimeLimits {
    pub turn_active_timeout_s: i64,
    pub cancel_confirm_timeout_s: i64,
    pub max_model_steps_per_turn: i64,
    pub max_parallel_workers: i64,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            turn_active_timeout_s: 1200,
            cancel_confirm_timeout_s: 60,
            max_model_steps_per_turn: 200,
            max_parallel_workers: 8,
        }
    }
}

type ToolSink = Box<dyn Fn(&str, &str, &Json) + Send + Sync>;
type JsonSink = Box<dyn Fn(&str, &Json) + Send + Sync>;
type RunnerCache = HashMap<String, (i64, Arc<dyn AgentRunner>)>;

type Sink = Box<dyn Fn(&str, &str, &str) + Send + Sync>;

/// Runtime-originated notifications: stream deltas to the UI, external backend
/// status/progress into the event log. Runners hold this, never the Runtime.
pub struct Notify {
    core: Arc<CoreClient>,
    stream: Mutex<Option<Sink>>,
    tool: Mutex<Option<ToolSink>>,
    /// Engine-level events (tool calls, turn ends, team actions) for user hooks.
    events: Mutex<Option<JsonSink>>,
    /// A member's plan changed (the UI shows it as a status component).
    plan: Mutex<Option<JsonSink>>,
    waker: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    accepting: Mutex<bool>,
}

impl Notify {
    pub fn new(core: Arc<CoreClient>) -> Arc<Self> {
        Arc::new(Self {
            core,
            stream: Mutex::new(None),
            tool: Mutex::new(None),
            events: Mutex::new(None),
            plan: Mutex::new(None),
            waker: Mutex::new(None),
            accepting: Mutex::new(true),
        })
    }

    pub fn set_stream_sink(&self, sink: Sink) {
        *self.stream.lock().unwrap() = Some(sink);
    }

    /// Tool activity (which tool, which arguments, ok or failed) for automation
    /// surfaces such as `exec --json`. The UI reads streamed text instead.
    pub fn set_tool_sink(&self, sink: ToolSink) {
        *self.tool.lock().unwrap() = Some(sink);
    }

    /// Independent of the UI sinks: whoever consumes tool activity (TUI, exec)
    /// may replace `tool`, but hooks must keep firing.
    pub fn set_event_sink(&self, sink: JsonSink) {
        *self.events.lock().unwrap() = Some(sink);
    }

    pub fn set_plan_sink(&self, sink: JsonSink) {
        *self.plan.lock().unwrap() = Some(sink);
    }

    pub fn note_plan(&self, agent_id: &str, items: &serde_json::Value) {
        if let Ok(plan) = self.plan.lock() {
            if let Some(sink) = plan.as_ref() {
                sink(agent_id, items);
            }
        }
    }

    pub fn note_event(&self, event: &str, payload: &serde_json::Value) {
        let accepting = self.accepting.lock().unwrap();
        if !*accepting {
            return;
        }
        if let Ok(events) = self.events.lock() {
            if let Some(sink) = events.as_ref() {
                sink(event, payload);
            }
        }
    }

    /// The core client behind this notifier. Runners read per-turn config
    /// (spec limits) and record follow-up state through it.
    pub fn core(&self) -> Arc<CoreClient> {
        self.core.clone()
    }

    pub fn set_waker(&self, waker: Box<dyn Fn() + Send + Sync>) {
        *self.waker.lock().unwrap() = Some(waker);
    }

    pub fn wake(&self) {
        if let Ok(waker) = self.waker.lock() {
            if let Some(waker) = waker.as_ref() {
                waker();
            }
        }
    }

    pub fn note_stream_chunk(&self, run_id: &str, agent_id: &str, text: &str) {
        let accepting = self.accepting.lock().unwrap();
        if !*accepting {
            return;
        }
        if text.is_empty() {
            return;
        }
        if let Ok(stream) = self.stream.lock() {
            if let Some(sink) = stream.as_ref() {
                sink(run_id, agent_id, text);
            }
        }
    }

    pub fn note_tool_activity(&self, run_id: &str, agent_id: &str, activity: &serde_json::Value) {
        let accepting = self.accepting.lock().unwrap();
        if !*accepting {
            return;
        }
        if let Ok(tool) = self.tool.lock() {
            if let Some(sink) = tool.as_ref() {
                sink(run_id, agent_id, activity);
            }
        }
    }

    /// Live status changes from an external backend.
    pub fn note_external_status(&self, run_id: &str, status: TurnStatus) {
        let accepting = self.accepting.lock().unwrap();
        if !*accepting {
            return;
        }
        let Ok(state) = self.core.state_brief() else { return };
        let runs: Vec<TurnRun> =
            serde_json::from_value(state.get("runs").cloned().unwrap_or(Json::Null)).unwrap_or_default();
        let Some(run) = runs.iter().find(|r| r.run_id == run_id) else { return };
        if run.status.is_terminal() {
            return;
        }
        let agent_id = run.agent_id.clone();
        core_best_effort(
            &self.core,
            "run status update",
            "set_run_status",
            json!({"run_id": run_id, "status": status}),
        );
        if status == TurnStatus::WaitingApproval {
            let pending: Vec<Json> =
                state.get("pending_approvals").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            if let Some(approval) = pending.iter().rfind(|a| a.get("run_id").and_then(|v| v.as_str()) == Some(run_id)) {
                core_best_effort(
                    &self.core,
                    "approval_requested event",
                    "emit",
                    json!({
                        "actor_id": agent_id,
                        "events": [{
                            "kind": "approval_requested",
                            "payload": {
                                "approval_id": approval.get("approval_id").cloned().unwrap_or(Json::Null),
                                "agent_id": agent_id,
                                "run_id": run_id,
                                "scope": approval.get("requested_scope").cloned().unwrap_or(Json::Null),
                            }
                        }],
                    }),
                );
            }
        }
        self.wake();
    }

    /// A human-readable progress line from a backend.
    pub fn note_external_progress(&self, run_id: &str, text: &str) {
        let accepting = self.accepting.lock().unwrap();
        if !*accepting {
            return;
        }
        if text.is_empty() {
            return;
        }
        let Ok(state) = self.core.state_brief() else { return };
        let runs: Vec<TurnRun> =
            serde_json::from_value(state.get("runs").cloned().unwrap_or(Json::Null)).unwrap_or_default();
        let Some(run) = runs.iter().find(|r| r.run_id == run_id) else { return };
        let requester = run.task_id.as_ref().and_then(|task_id| {
            state
                .get("tasks")
                .and_then(|v| v.as_array())
                .and_then(|tasks| {
                    tasks.iter().find(|t| t.get("task_id").and_then(|v| v.as_str()) == Some(task_id.as_str()))
                })
                .and_then(|task| task.get("requester"))
                .cloned()
        });
        core_best_effort(
            &self.core,
            "run_progress event",
            "emit",
            json!({
                "actor_id": run.agent_id,
                "events": [{
                    "kind": "run_progress",
                    "payload": {
                        "run_id": run_id,
                        "agent_id": run.agent_id,
                        "text": text.chars().take(2000).collect::<String>(),
                        "requester": requester,
                        "task_id": run.task_id,
                    }
                }],
            }),
        );
        self.wake();
    }
}

struct RunSlot {
    agent_id: String,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    cancel_started: AtomicBool,
    control: Arc<TurnControl>,
}

#[derive(Clone)]
struct PendingFinalization {
    run: TurnRun,
    outcome: TurnOutcome,
    ack_ids: Vec<i64>,
    retry_after: Instant,
}

struct RuntimeError {
    phase: &'static str,
    run_id: Option<String>,
    agent_id: Option<String>,
    error: String,
    retry_after: Instant,
}

pub struct Runtime {
    pub core: Arc<CoreClient>,
    pub notify: Arc<Notify>,
    runners: Mutex<RunnerCache>,
    factory: Option<RunnerFactory>,
    inflight: Mutex<HashMap<String, Arc<RunSlot>>>,
    // Retry the core transaction, never a member that has already returned.
    // Cold recovery still uses the backend's durable checkpoint/turn identity.
    finalizing: Mutex<HashMap<String, PendingFinalization>>,
    // Transient diagnostics are not a second source of execution state.
    errors: Mutex<HashMap<String, RuntimeError>>,
    reconciliation_pending: AtomicBool,
    approvals: Arc<ApprovalGate>,
    executor: ToolExecutor,
    // Keep core drain order through backend handoff across submit/loop threads.
    mid_turn_delivery: Mutex<()>,
    offered: Mutex<HashMap<String, HashSet<i64>>>,
    steps: Arc<Mutex<HashMap<String, i64>>>,
    limits: RuntimeLimits,
    closed: AtomicBool,
    wake: (Mutex<bool>, Condvar),
    loop_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    retiring: Mutex<Vec<std::thread::JoinHandle<()>>>,
    self_ref: Mutex<Weak<Runtime>>,
    /// D-30: installed by the session; runs before an apply_topology_patch
    /// submit (auto-creates per-member model profiles).
    topology_prepare: Mutex<Option<crate::gateway::TopologyPrepare>>,
    /// `[hooks] pre_tool`: user policy each tool gateway consults before running.
    hooks: Mutex<Option<Arc<crate::hooks::Hooks>>>,
}

impl Runtime {
    pub fn new(
        core: Arc<CoreClient>,
        notify: Arc<Notify>,
        approvals: Arc<ApprovalGate>,
        executor: ToolExecutor,
        factory: Option<RunnerFactory>,
        limits: RuntimeLimits,
    ) -> Arc<Self> {
        let runtime = Arc::new(Self {
            core,
            notify,
            runners: Mutex::new(HashMap::new()),
            factory,
            inflight: Mutex::new(HashMap::new()),
            finalizing: Mutex::new(HashMap::new()),
            errors: Mutex::new(HashMap::new()),
            reconciliation_pending: AtomicBool::new(true),
            approvals,
            executor,
            mid_turn_delivery: Mutex::new(()),
            offered: Mutex::new(HashMap::new()),
            steps: Arc::new(Mutex::new(HashMap::new())),
            limits,
            closed: AtomicBool::new(false),
            wake: (Mutex::new(false), Condvar::new()),
            loop_thread: Mutex::new(None),
            retiring: Mutex::new(vec![]),
            self_ref: Mutex::new(Weak::new()),
            topology_prepare: Mutex::new(None),
            hooks: Mutex::new(None),
        });
        *runtime.self_ref.lock().unwrap() = Arc::downgrade(&runtime);
        let weak = Arc::downgrade(&runtime);
        runtime.notify.set_waker(Box::new(move || {
            if let Some(rt) = weak.upgrade() {
                rt.signal();
            }
        }));
        runtime
    }

    pub fn set_hooks(&self, hooks: Arc<crate::hooks::Hooks>) {
        *self.hooks.lock().unwrap() = Some(hooks);
    }

    pub fn set_topology_prepare(&self, hook: crate::gateway::TopologyPrepare) {
        *self.topology_prepare.lock().unwrap() = Some(hook);
    }

    pub fn add_runner(&self, agent_id: &str, runner: Arc<dyn AgentRunner>) {
        let revision = self
            .core
            .state_brief()
            .ok()
            .and_then(|s| s["agents"].as_array()?.iter().find(|a| a["id"] == agent_id)?["config_revision"].as_i64())
            .unwrap_or(0);
        self.runners.lock().unwrap().insert(agent_id.to_string(), (revision, runner));
    }

    pub fn runner(&self, agent_id: &str) -> Option<Arc<dyn AgentRunner>> {
        self.runners.lock().unwrap().get(agent_id).map(|(_, runner)| runner.clone())
    }

    /// Drop the cached runner so the next turn rebuilds it from the factory
    /// (session-level model override). Same remove+close pattern as the
    /// stale-rebuild path in start_ready_runs. An active runner stays reachable
    /// for cancellation, approval decisions and mid-turn input until it exits.
    pub fn drop_runner(&self, agent_id: &str) {
        let old = {
            let inflight = self.inflight.lock().unwrap();
            let mut runners = self.runners.lock().unwrap();
            if inflight.values().any(|slot| slot.agent_id == agent_id) {
                if let Some((revision, _)) = runners.get_mut(agent_id) {
                    *revision = -1;
                }
                return;
            }
            runners.remove(agent_id)
        };
        if let Some((_, runner)) = old {
            runner.close();
        }
    }

    pub fn me(&self) -> Option<Arc<Runtime>> {
        self.self_ref.lock().unwrap().upgrade()
    }

    fn signal(&self) {
        let (lock, cv) = &self.wake;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }

    fn sleep_or_wake(&self, ms: u64) {
        let (lock, cv) = &self.wake;
        let mut flag = lock.lock().unwrap();
        if !*flag {
            let (guard, _) = cv.wait_timeout(flag, Duration::from_millis(ms)).unwrap();
            flag = guard;
        }
        *flag = false;
    }

    // -- lifecycle -----------------------------------------------------------

    pub fn start(self: &Arc<Self>) {
        self.closed.store(false, Ordering::SeqCst);
        self.reconcile();
        let runtime = self.clone();
        let handle = std::thread::spawn(move || runtime.run_loop());
        *self.loop_thread.lock().unwrap() = Some(handle);
    }

    /// Revoke turns before releasing ownership. Model HTTP calls may finish
    /// later, but their tools/checkpoint writes can no longer enter the gate.
    pub fn close(&self) {
        let slots: Vec<Arc<RunSlot>> = {
            // Keep wrappers from removing their tool scope before we drain it.
            let inflight = self.inflight.lock().unwrap();
            self.closed.store(true, Ordering::SeqCst);
            let slots: Vec<_> = inflight.values().cloned().collect();
            for slot in &slots {
                slot.control.cancel();
            }
            slots
        };
        self.signal();
        if let Some(handle) = self.loop_thread.lock().unwrap().take() {
            let _ = handle.join();
        }
        *self.notify.accepting.lock().unwrap() = false;
        let runners: Vec<Arc<dyn AgentRunner>> =
            self.runners.lock().unwrap().values().map(|(_, r)| r.clone()).collect();
        for runner in runners {
            runner.close();
        }
        let retiring = std::mem::take(&mut *self.retiring.lock().unwrap());
        for handle in retiring {
            let _ = handle.join();
        }
        for slot in slots {
            while !slot.control.wait_idle(Duration::from_millis(50)) {}
            if let Some(handle) = slot.handle.lock().unwrap().take() {
                let _ = handle.join();
            }
        }
    }

    fn run_loop(&self) {
        while !self.closed.load(Ordering::SeqCst) {
            self.start_ready_runs();
            self.drain_mid_turn();
            self.sleep_or_wake(50);
        }
    }

    // -- ingress -------------------------------------------------------------

    pub fn submit(&self, action: TeamAction) -> Result<Receipt, String> {
        if self.reconciliation_pending.load(Ordering::SeqCst) {
            // exec accepts new input before start(), and recovery may be
            // waiting on storage. Neither path may schedule unchecked intents.
            self.prepare_reconciliation()?;
        }
        let receipt = self.core.submit(&action)?;
        if action.kind == teamagents_core::models::ActionKind::ApprovalDecision && receipt.ok {
            let payload = &action.payload;
            let approval_id = payload.get("approval_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let decision = payload.get("decision").and_then(|v| v.as_str()).unwrap_or("").to_string();
            self.deliver_approval_decision(&approval_id, &decision);
        }
        // team actions (assign_task / complete_task / signal_done / patches …)
        // are visible to hooks as one stream with their receipts
        if action.actor_id != "user" || action.kind == teamagents_core::models::ActionKind::UserMessage {
            self.notify.note_event(
                "team_action",
                &json!({
                    "kind": action.kind,
                    "actor_id": action.actor_id,
                    "action_id": action.action_id,
                    "ok": receipt.ok,
                    "error": receipt.error,
                    "payload": action.payload,
                }),
            );
        }
        self.drain_mid_turn();
        self.signal();
        Ok(receipt)
    }

    pub fn user_message(&self, text: &str, supplement: bool) -> Result<Receipt, String> {
        let action = TeamAction {
            action_id: teamagents_core::models::new_id("user"),
            session_id: self.core.session_id.clone(),
            actor_id: "user".into(),
            run_id: None,
            kind: if supplement {
                teamagents_core::models::ActionKind::UserSupplement
            } else {
                teamagents_core::models::ActionKind::UserMessage
            },
            payload: json!({"text": text}),
        };
        self.submit(action)
    }

    fn deliver_approval_decision(&self, approval_id: &str, decision: &str) {
        let Ok(reply) = self.core.call_in_session("get_approval", json!({"approval_id": approval_id})) else { return };
        let approval = reply.get("approval").cloned().unwrap_or(Json::Null);
        let Some(run_id) = approval.get("run_id").and_then(|v| v.as_str()) else { return };
        let run_id = run_id.to_string();
        let Ok(state) = self.core.state_brief() else { return };
        let runs: Vec<TurnRun> =
            serde_json::from_value(state.get("runs").cloned().unwrap_or(Json::Null)).unwrap_or_default();
        let Some(run) = runs.iter().find(|r| r.run_id == run_id) else { return };
        if let Some(runner) = self.runner(&run.agent_id) {
            // false = the backend has no waiter for it (the wait timed out and
            // the approval was already voided, or the run moved on). At least
            // leave a trace instead of dropping the user's decision silently.
            if !runner.resolve_approval(approval_id, decision) && runner.has_approval_waiter() {
                eprintln!(
                    "approval {approval_id} decided as {decision} but member {} has no waiter for it",
                    run.agent_id
                );
            }
        }
    }

    fn drain_mid_turn(&self) {
        let _delivery = self.mid_turn_delivery.lock().unwrap();
        if self.closed.load(Ordering::SeqCst) || !self.retry_due("delivery", "") {
            return;
        }
        let pushes = match self.core.drain_mid_turn() {
            Ok(pushes) => {
                self.clear_error("delivery", "");
                pushes
            }
            Err(error) => {
                // The core transaction leaves all batches buffered on failure.
                // Retry from the loop even without another user/tool action.
                self.record_error("delivery", None, error);
                return;
            }
        };
        for push in pushes {
            if let Some(runner) = self.runner(&push.agent_id) {
                let ids: Vec<i64> = push.items.iter().filter_map(|i| i["delivery_id"].as_i64()).collect();
                self.offered.lock().unwrap().entry(push.run_id.clone()).or_default().extend(ids);
                runner.deliver_mid_turn(&push.run_id, push.items);
            }
        }
    }

    /// Full session snapshot including events. Internal hot paths use
    /// `core.state_brief()` instead (P2-9).
    pub fn state(&self) -> Result<Json, String> {
        let mut state = self.core.state()?;
        state["runtime_errors"] = json!(self.errors());
        Ok(state)
    }

    /// User-visible reasons why durable work is waiting for storage recovery.
    /// Keep this independent of SQLite: a failed write cannot record its own
    /// diagnostic there. Stable fields let polling surfaces deduplicate it.
    pub fn errors(&self) -> Vec<Json> {
        let errors = self.errors.lock().unwrap();
        let mut ordered: Vec<_> = errors.iter().collect();
        ordered.sort_by(|a, b| a.0.cmp(b.0));
        ordered
            .into_iter()
            .map(|(_, error)| {
                json!({
                    "phase": error.phase, "run_id": error.run_id, "agent_id": error.agent_id, "error": error.error,
                })
            })
            .collect()
    }

    fn record_error(&self, phase: &'static str, run: Option<&TurnRun>, error: String) {
        let run_id = run.map(|run| run.run_id.as_str()).unwrap_or("");
        let key = format!("{phase}:{run_id}");
        let mut errors = self.errors.lock().unwrap();
        if errors.get(&key).is_none_or(|previous| previous.error != error) {
            eprintln!("teamagents: 运行时暂缓处理（{phase} {run_id}），将重试：{error}");
        }
        errors.insert(
            key,
            RuntimeError {
                phase,
                run_id: run.map(|run| run.run_id.clone()),
                agent_id: run.map(|run| run.agent_id.clone()),
                error,
                retry_after: Instant::now() + Duration::from_secs(1),
            },
        );
    }

    fn clear_error(&self, phase: &str, run_id: &str) {
        self.errors.lock().unwrap().remove(&format!("{phase}:{run_id}"));
    }

    fn retry_due(&self, phase: &str, run_id: &str) -> bool {
        self.errors
            .lock()
            .unwrap()
            .get(&format!("{phase}:{run_id}"))
            .is_none_or(|error| Instant::now() >= error.retry_after)
    }

    // -- scheduler -----------------------------------------------------------

    /// State only lists current members: absence from the committed spec is
    /// the removal signal, not a REMOVED row in state.agents. A draining member
    /// remains in the spec until its boundary, and its wrapper must exit before
    /// we close the runner (finalization still consults its delivery checkpoint).
    fn retire_removed_runners(&self, members: &HashSet<&str>) {
        let removed = {
            let inflight = self.inflight.lock().unwrap();
            let mut runners = self.runners.lock().unwrap();
            let ids: Vec<_> = runners
                .keys()
                .filter(|id| !members.contains(id.as_str()) && !inflight.values().any(|slot| slot.agent_id == **id))
                .cloned()
                .collect();
            ids.into_iter().filter_map(|id| runners.remove(&id).map(|(_, runner)| runner)).collect::<Vec<_>>()
        };
        let finished = {
            let mut retiring = self.retiring.lock().unwrap();
            let mut finished = vec![];
            let mut index = 0;
            while index < retiring.len() {
                if retiring[index].is_finished() {
                    finished.push(retiring.swap_remove(index));
                } else {
                    index += 1;
                }
            }
            // MCP HTTP shutdown may wait for the peer. Never hold scheduler or
            // runner locks while closing a backend; session.close joins these.
            for runner in removed {
                retiring.push(std::thread::spawn(move || runner.close()));
            }
            finished
        };
        for handle in finished {
            let _ = handle.join();
        }
    }

    fn start_ready_runs(&self) {
        self.retry_finalizations();
        if self.reconciliation_pending.load(Ordering::SeqCst) {
            if self.retry_due("reconcile", "") {
                self.reconcile();
            }
            if self.reconciliation_pending.load(Ordering::SeqCst) {
                return;
            }
        }
        if !self.retry_due("state", "") {
            return;
        }
        let state = match self.core.state_brief() {
            Ok(state) => {
                self.clear_error("state", "");
                state
            }
            Err(error) => {
                self.record_error("state", None, error);
                return;
            }
        };
        let session = state.get("session").cloned().unwrap_or(Json::Null);
        if session.is_null() {
            return;
        }
        let Some(agents) = state.pointer("/spec/agents").and_then(Json::as_array) else { return };
        let members: HashSet<&str> = agents.iter().filter_map(|a| a["id"].as_str()).collect();
        self.retire_removed_runners(&members);
        let runs: Vec<TurnRun> =
            serde_json::from_value(state.get("runs").cloned().unwrap_or(Json::Null)).unwrap_or_default();
        self.errors.lock().unwrap().retain(|_, error| {
            error.phase != "prepare"
                || runs.iter().any(|run| error.run_id.as_deref() == Some(run.run_id.as_str()) && run.status.is_active())
        });
        if session.get("status").and_then(|v| v.as_str()) == Some("PAUSED") {
            // Pausing stops dispatch, but must not suppress requests to stop work.
            self.watch_cancellations(&runs);
            return;
        }
        let leader_id = state.get("leader_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let limit_workers = state
            .get("limits")
            .and_then(|l| l.get("max_parallel_workers"))
            .and_then(|v| v.as_i64())
            .unwrap_or(self.limits.max_parallel_workers);
        let mut workers_busy = 0i64;
        for run_id in self.inflight.lock().unwrap().keys() {
            if let Some(run) = runs.iter().find(|r| &r.run_id == run_id) {
                if run.agent_id != leader_id && run.status.is_active() {
                    workers_busy += 1;
                }
            }
        }
        let active: Vec<TurnRun> = runs.iter().filter(|r| r.status.is_active()).cloned().collect();
        for run in active {
            if !members.contains(run.agent_id.as_str())
                || self.inflight.lock().unwrap().contains_key(&run.run_id)
                || self.finalizing.lock().unwrap().contains_key(&run.run_id)
                || !self.retry_due("prepare", &run.run_id)
            {
                continue;
            }
            if self.factory.is_some() {
                let stale = self
                    .runners
                    .lock()
                    .unwrap()
                    .get(&run.agent_id)
                    .map(|(revision, _)| *revision != run.config_revision)
                    .unwrap_or(false);
                if stale {
                    let old = self.runners.lock().unwrap().remove(&run.agent_id);
                    if let Some((_, runner)) = old {
                        runner.close();
                    }
                }
            }
            let runner = match self.runner(&run.agent_id) {
                Some(runner) => Some(runner),
                None => {
                    let built = match (self.factory.as_ref(), self.spec_for_agent(&run.agent_id)) {
                        (Some(factory), Some(spec)) => Some(factory(&spec)),
                        _ => None,
                    };
                    match built {
                        Some(Ok(runner)) => {
                            self.add_runner(&run.agent_id, runner.clone());
                            Some(runner)
                        }
                        Some(Err(e)) => {
                            self.finalize(
                                &run,
                                TurnOutcome {
                                    status: TurnStatus::Failed,
                                    error: Some(format!("cannot start member {}: {e}", run.agent_id)),
                                    note: None,
                                    reply_text: None,
                                },
                            );
                            continue;
                        }
                        None => None,
                    }
                }
            };
            let Some(runner) = runner else { continue };
            if run.agent_id != leader_id && workers_busy >= limit_workers {
                continue;
            }
            if run.agent_id != leader_id {
                workers_busy += 1;
            }
            self.spawn_execute(run, runner);
        }
        self.watch_cancellations(&runs);
    }

    fn spec_for_agent(&self, agent_id: &str) -> Option<AgentSpec> {
        let state = self.core.state_brief().ok()?;
        let spec = state.get("spec")?.clone();
        let agents: Vec<AgentSpec> = serde_json::from_value(spec.get("agents")?.clone()).ok()?;
        agents.into_iter().find(|a| a.id == agent_id)
    }

    fn spawn_execute(&self, run: TurnRun, _runner: Arc<dyn AgentRunner>) {
        let Some(runtime) = self.me() else { return };
        let slot = Arc::new(RunSlot {
            agent_id: run.agent_id.clone(),
            handle: Mutex::new(None),
            cancel_started: AtomicBool::new(false),
            control: Arc::new(TurnControl::default()),
        });
        {
            let mut inflight = self.inflight.lock().unwrap();
            if self.closed.load(Ordering::SeqCst) {
                return;
            }
            inflight.insert(run.run_id.clone(), slot.clone());
        }
        let run_id = run.run_id.clone();
        let control = slot.control.clone();
        let handle = std::thread::spawn(move || {
            runtime.execute(&run, &control);
            runtime.inflight.lock().unwrap().remove(&run_id);
            runtime.signal();
        });
        *slot.handle.lock().unwrap() = Some(handle);
    }

    fn watch_cancellations(&self, runs: &[TurnRun]) {
        let mut stopping: Vec<(TurnRun, Arc<dyn AgentRunner>)> = vec![];
        for (run_id, slot) in self.inflight.lock().unwrap().iter() {
            if slot.cancel_started.load(Ordering::SeqCst) {
                continue;
            }
            let Some(run) = runs.iter().find(|r| &r.run_id == run_id) else { continue };
            if !run.cancel_requested {
                continue;
            }
            slot.control.cancel();
            let Some(runner) = self.runner(&run.agent_id) else { continue };
            slot.cancel_started.store(true, Ordering::SeqCst);
            stopping.push((run.clone(), runner));
        }
        for (run, runner) in stopping {
            let Some(runtime) = self.me() else { return };
            let runtime: Arc<Runtime> = runtime;
            std::thread::spawn(move || runtime.request_stop(&run, runner));
        }
    }

    fn request_stop(&self, run: &TurnRun, runner: Arc<dyn AgentRunner>) {
        let timeout = Duration::from_secs(self.limits.cancel_confirm_timeout_s.max(1) as u64);
        let (tx, rx) = channel();
        let run_id = run.run_id.clone();
        let interrupt_runner = runner.clone();
        std::thread::spawn(move || {
            let _ = tx.send(interrupt_runner.request_interrupt(&run_id));
        });
        // confirmation timeout: mark OUTCOME_UNKNOWN + expire approvals (RT-06)
        if let Err(RecvTimeoutError::Timeout) = rx.recv_timeout(timeout) {
            if self.closed.load(Ordering::SeqCst) {
                return;
            }
            core_best_effort(&self.core, "stop timeout bookkeeping", "stop_timeout", json!({"run_id": run.run_id}));
        }
        self.signal();
    }

    // -- executor ------------------------------------------------------------

    fn execute(&self, run: &TurnRun, control: &Arc<TurnControl>) {
        let guard = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.execute_inner(run, control)));
        match guard {
            Ok(Ok(())) => {}
            Ok(Err(e)) => self.finalize(
                run,
                TurnOutcome { status: TurnStatus::Failed, error: Some(e), note: None, reply_text: None },
            ),
            Err(panic) => {
                let message = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".into());
                self.finalize(
                    run,
                    TurnOutcome {
                        status: TurnStatus::Failed,
                        error: Some(format!("panic: {message}")),
                        note: None,
                        reply_text: None,
                    },
                );
            }
        }
        self.offered.lock().unwrap().remove(&run.run_id);
        self.steps.lock().unwrap().remove(&run.run_id);
        self.signal();
    }

    fn execute_inner(&self, run: &TurnRun, control: &Arc<TurnControl>) -> Result<(), String> {
        if self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        let Some(runner) = self.runner(&run.agent_id) else {
            self.finalize(
                run,
                TurnOutcome {
                    status: TurnStatus::Failed,
                    error: Some(format!("no runner for member {}", run.agent_id)),
                    note: None,
                    reply_text: None,
                },
            );
            return Ok(());
        };
        let begin = match self.core.call_in_session("prepare_run", json!({"run_id": run.run_id})) {
            Ok(begin) => {
                self.clear_error("prepare", &run.run_id);
                begin
            }
            Err(error) => {
                // No runner has been entered. The atomic preparation rolled
                // back, so retry this intent without ending or replacing it.
                self.record_error("prepare", Some(run), error);
                return Ok(());
            }
        };
        let fresh: TurnRun = serde_json::from_value(begin.get("run").cloned().unwrap_or(Json::Null))
            .map_err(|e| format!("bad run: {e}"))?;
        let wake = begin.get("wake").cloned().unwrap_or(Json::Null);
        let view = begin.get("view").cloned().ok_or("prepared run lacks its member view")?;
        let delivery_ids: Vec<i64> =
            view.get("delivery_ids").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
        if !delivery_ids.is_empty() {
            let mut offered = self.offered.lock().unwrap();
            let slot = offered.entry(fresh.run_id.clone()).or_default();
            for id in delivery_ids {
                slot.insert(id);
            }
        }
        let limits = self.effective_limits();
        let timeout = Duration::from_secs(limits.turn_active_timeout_s.max(1) as u64);
        let gateway = ToolGateway::with_control(
            self.core.clone(),
            &fresh.agent_id,
            &fresh.run_id,
            self.approvals.clone(),
            Some(self.guarded_executor(
                &fresh.agent_id,
                &fresh.run_id,
                limits.max_model_steps_per_turn,
                control.clone(),
            )),
            control.clone(),
            self.topology_prepare.lock().unwrap().clone(),
            self.hooks.lock().unwrap().clone(),
        );
        let mut outcome = self.run_with_timeout(runner.clone(), &fresh, &view, gateway, &wake, timeout);

        // The backend has returned and may have committed external effects.
        // Retain its exact outcome while the cancellation read is unavailable;
        // never replace it with an infrastructure error or enter the runner again.
        let runs = loop {
            if self.closed.load(Ordering::SeqCst) {
                return Ok(());
            }
            if !self.retry_due("outcome", &fresh.run_id) {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            let result = self.core.state_brief().and_then(|state| {
                serde_json::from_value::<Vec<TurnRun>>(state.get("runs").cloned().unwrap_or(Json::Null))
                    .map_err(|error| format!("读取回合状态失败：{error}"))
            });
            match result {
                Ok(runs) => {
                    self.clear_error("outcome", &fresh.run_id);
                    break runs;
                }
                Err(error) => self.record_error("outcome", Some(&fresh), error),
            }
        };
        let cancel_requested =
            runs.iter().find(|r| r.run_id == fresh.run_id).map(|r| r.cancel_requested).unwrap_or(false);
        if cancel_requested
            && matches!(outcome.status, TurnStatus::Completed | TurnStatus::WaitingTask | TurnStatus::WaitingApproval)
        {
            let confirmed = self.interrupt_confirm(&fresh.run_id, runner);
            if confirmed != outcome.status {
                outcome = TurnOutcome {
                    status: confirmed,
                    error: None,
                    note: Some("cancelled by request".into()),
                    reply_text: outcome.reply_text,
                };
            }
        }
        self.finalize(&fresh, outcome);
        Ok(())
    }

    /// Run the member on its own thread so the active-time limit can fire.
    /// On timeout the member is interrupted: without it the thread keeps
    /// calling the model and writing team state after the run is already FAILED.
    fn run_with_timeout(
        &self,
        runner: Arc<dyn AgentRunner>,
        run: &TurnRun,
        view: &Json,
        gateway: Arc<ToolGateway>,
        wake: &Json,
        timeout: Duration,
    ) -> TurnOutcome {
        let (tx, rx) = channel();
        let (run_clone, view_clone, wake_clone) = (run.clone(), view.clone(), wake.clone());
        let runner_for_interrupt = runner.clone();
        let control = gateway.control.clone();
        std::thread::spawn(move || {
            let outcome = runner.start_or_resume(&run_clone, &view_clone, &gateway, &wake_clone);
            let _ = tx.send(outcome);
        });
        let deadline = Instant::now() + timeout;
        loop {
            if self.closed.load(Ordering::SeqCst) {
                return TurnOutcome { status: TurnStatus::Cancelled, error: None, note: None, reply_text: None };
            }
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(outcome) => return outcome,
                Err(RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                    control.cancel();
                    let run_id = run.run_id.clone();
                    std::thread::spawn(move || {
                        runner_for_interrupt.request_interrupt(&run_id);
                    });
                    let stopped = control
                        .wait_idle(Duration::from_secs(self.effective_limits().cancel_confirm_timeout_s.max(1) as u64));
                    return TurnOutcome {
                        status: if stopped { TurnStatus::Failed } else { TurnStatus::OutcomeUnknown },
                        error: Some(format!("turn active-time limit {}s reached", timeout.as_secs())),
                        note: None,
                        reply_text: None,
                    };
                }
                Err(RecvTimeoutError::Timeout) => continue,
                // the member thread panicked before sending: never report that as a
                // timeout (a 1200s claim for an immediate crash sends debugging the
                // wrong way)
                Err(RecvTimeoutError::Disconnected) => {
                    return TurnOutcome {
                        status: TurnStatus::Failed,
                        error: Some("member runner crashed before reporting an outcome".into()),
                        note: None,
                        reply_text: None,
                    }
                }
            }
        }
    }

    fn interrupt_confirm(&self, run_id: &str, runner: Arc<dyn AgentRunner>) -> TurnStatus {
        let timeout = Duration::from_secs(self.limits.cancel_confirm_timeout_s.max(1) as u64);
        let (tx, rx) = channel();
        let run_id_owned = run_id.to_string();
        std::thread::spawn(move || {
            let _ = tx.send(runner.request_interrupt(&run_id_owned));
        });
        match rx.recv_timeout(timeout) {
            Ok(status) => status,
            Err(_) => TurnStatus::OutcomeUnknown,
        }
    }

    /// What the gateway calls: (tool, args), with the step budget applied and
    /// the session-level executor addressed by agent (per-member roots).
    fn guarded_executor(
        &self,
        agent_id: &str,
        run_id: &str,
        max_steps: i64,
        control: Arc<TurnControl>,
    ) -> crate::gateway::Executor {
        let steps = self.steps.clone();
        let base = self.executor.clone();
        let run_id = run_id.to_string();
        let agent_id = agent_id.to_string();
        Arc::new(move |tool: &str, args: &Json| {
            let used = {
                let mut counts = steps.lock().unwrap();
                let entry = counts.entry(run_id.clone()).or_insert(0);
                *entry += 1;
                *entry
            };
            if used > max_steps {
                return Err(format!("step limit {max_steps} reached for this turn"));
            }
            base(&agent_id, tool, args, &control)
        })
    }

    fn effective_limits(&self) -> RuntimeLimits {
        let Ok(state) = self.core.state_brief() else { return self.limits.clone() };
        let field = |name: &str, fallback: i64| -> i64 {
            state.get("limits").and_then(|l| l.get(name)).and_then(|v| v.as_i64()).unwrap_or(fallback)
        };
        RuntimeLimits {
            turn_active_timeout_s: field("turn_active_timeout_s", self.limits.turn_active_timeout_s),
            cancel_confirm_timeout_s: field("cancel_confirm_timeout_s", self.limits.cancel_confirm_timeout_s),
            max_model_steps_per_turn: field("max_model_steps_per_turn", self.limits.max_model_steps_per_turn),
            max_parallel_workers: field("max_parallel_workers", self.limits.max_parallel_workers),
        }
    }

    fn finalize(&self, run: &TurnRun, outcome: TurnOutcome) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let mut ack: Vec<i64> =
            self.offered.lock().unwrap().remove(&run.run_id).map(|ids| ids.into_iter().collect()).unwrap_or_default();
        if let Some(applied) = self.runner(&run.agent_id).and_then(|r| r.applied_delivery_ids(run)) {
            ack.retain(|id| applied.contains(id));
        }
        self.finalizing.lock().unwrap().entry(run.run_id.clone()).or_insert(PendingFinalization {
            run: run.clone(),
            outcome,
            ack_ids: ack,
            retry_after: Instant::now(),
        });
        self.retry_finalizations();
        self.signal();
    }

    fn retry_finalizations(&self) {
        let mut committed = vec![];
        {
            // Serialize attempts with immediate finalization so parked segments
            // cannot emit duplicate events. Keep the original receipt and input
            // acknowledgements until the entire core transaction commits.
            let mut pending = self.finalizing.lock().unwrap();
            if self.closed.load(Ordering::SeqCst) {
                return;
            }
            pending.retain(|_, item| {
                if Instant::now() < item.retry_after {
                    return true;
                }
                let result = self
                    .core
                    .call_in_session(
                        "finalize_run",
                        json!({
                            "run_id": item.run.run_id,
                            "status": item.outcome.status,
                            "error": item.outcome.error,
                            "note": item.outcome.note,
                            "reply_text": item.outcome.reply_text,
                            "ack_ids": item.ack_ids,
                        }),
                    )
                    .and_then(|value| {
                        serde_json::from_value::<FinalizationResult>(value)
                            .map_err(|error| format!("invalid finalization result: {error}"))
                    });
                match result {
                    Ok(result) => {
                        committed.push((item.clone(), result));
                        false
                    }
                    Err(error) => {
                        self.record_error("finalize", Some(&item.run), error);
                        item.retry_after = Instant::now() + Duration::from_secs(1);
                        true
                    }
                }
            });
        }
        for (item, result) in &committed {
            self.clear_error("finalize", &item.run.run_id);
            if !result.applied {
                continue;
            }
            let event = match result.status {
                TurnStatus::Completed => "run_completed",
                TurnStatus::Failed | TurnStatus::OutcomeUnknown => "run_failed",
                TurnStatus::Cancelled => "run_cancelled",
                TurnStatus::WaitingTask | TurnStatus::WaitingApproval => "run_paused",
                TurnStatus::Queued | TurnStatus::Running => continue,
            };
            // A late decision/cancellation can replace a parked segment inside
            // the core transaction. Its old payload does not describe that state.
            let same_outcome = result.status == item.outcome.status;
            self.notify.note_event(
                event,
                &json!({
                    "run_id": item.run.run_id,
                    "agent_id": item.run.agent_id,
                    "status": result.status,
                    "error": same_outcome.then_some(item.outcome.error.as_deref()).flatten(),
                    "reply_text": same_outcome.then_some(item.outcome.reply_text.as_deref()).flatten(),
                }),
            );
        }
        if !committed.is_empty() {
            self.drain_mid_turn();
            self.signal();
        }
    }

    // -- restart convergence (RT-04) ------------------------------------------

    /// After a restart: re-check in-flight runs, never blind-retry side effects.
    pub fn reconcile(&self) {
        match self.reconcile_inner() {
            Ok(()) => {
                self.reconciliation_pending.store(false, Ordering::SeqCst);
                self.clear_error("reconcile", "");
            }
            Err(error) => {
                self.reconciliation_pending.store(true, Ordering::SeqCst);
                self.record_error("reconcile", None, error);
            }
        }
    }

    pub(crate) fn prepare_reconciliation(&self) -> Result<Vec<TurnRun>, String> {
        let state = self.core.state_brief()?;
        let mut runs: Vec<TurnRun> =
            serde_json::from_value(state["runs"].clone()).map_err(|error| format!("invalid recovery runs: {error}"))?;
        let mut recover = vec![];
        let queued_ids: Vec<String> = runs
            .iter()
            .filter(|run| {
                run.status == TurnStatus::Queued
                    && !self.inflight.lock().unwrap().contains_key(&run.run_id)
                    && !self.finalizing.lock().unwrap().contains_key(&run.run_id)
            })
            .map(|run| run.run_id.clone())
            .collect();
        let started_ids = self.core.run_started_ids(&queued_ids)?;
        for run in &runs {
            if run.status != TurnStatus::Queued
                || self.inflight.lock().unwrap().contains_key(&run.run_id)
                || self.finalizing.lock().unwrap().contains_key(&run.run_id)
            {
                continue;
            }
            if let Some(runner) = self.runner(&run.agent_id) {
                if started_ids.contains(&run.run_id) || runner.has_recovery_state(run)? {
                    recover.push(run.run_id.clone());
                }
            }
        }
        if !recover.is_empty() {
            let restored: HashMap<_, _> =
                self.core.restore_queued_runs(&recover)?.into_iter().map(|run| (run.run_id.clone(), run)).collect();
            for run in &mut runs {
                if let Some(fresh) = restored.get(&run.run_id) {
                    *run = fresh.clone();
                }
            }
        }
        Ok(runs)
    }

    fn reconcile_inner(&self) -> Result<(), String> {
        let runs = self.prepare_reconciliation()?;
        let parked: Vec<TurnRun> = runs
            .iter()
            .filter(|r| matches!(r.status, TurnStatus::Running | TurnStatus::WaitingTask | TurnStatus::WaitingApproval))
            .cloned()
            .collect();
        for run in parked {
            if self.finalizing.lock().unwrap().contains_key(&run.run_id)
                || self.inflight.lock().unwrap().contains_key(&run.run_id)
            {
                continue;
            }
            let runner = self.runner(&run.agent_id);
            let mut outcome = runner.as_ref().and_then(|r| r.query_state(&run.run_id)).map(|status| TurnOutcome {
                status,
                error: None,
                note: None,
                reply_text: None,
            });
            if outcome.is_none() {
                if let Some(runner) = &runner {
                    let gateway =
                        ToolGateway::new(self.core.clone(), &run.agent_id, &run.run_id, self.approvals.clone(), None);
                    outcome = runner.reconcile(&run, &gateway);
                }
            }
            let status = outcome.as_ref().map(|o| o.status);
            if status == Some(TurnStatus::Queued) {
                self.core.call_in_session("requeue_run", json!({"run_id": run.run_id}))?;
                continue;
            }
            if run.status != TurnStatus::Running {
                if status == Some(run.status) {
                    continue;
                }
                match outcome {
                    Some(o) => self.converge(&run, o),
                    None => self.converge(
                        &run,
                        TurnOutcome {
                            status: TurnStatus::OutcomeUnknown,
                            error: Some("suspended turn could not be restored after restart".into()),
                            note: None,
                            reply_text: None,
                        },
                    ),
                }
                continue;
            }
            if let Some(o) = outcome {
                if o.status.is_terminal() || matches!(o.status, TurnStatus::WaitingTask | TurnStatus::WaitingApproval) {
                    self.converge(&run, o);
                }
                continue;
            }
            if run.external_turn_id.is_some() {
                self.converge(
                    &run,
                    TurnOutcome {
                        status: TurnStatus::OutcomeUnknown,
                        error: Some("external turn outcome could not be confirmed".into()),
                        note: None,
                        reply_text: None,
                    },
                );
            } else {
                // in-process runner only: safe to re-run the segment
                self.core.call_in_session("requeue_run", json!({"run_id": run.run_id}))?;
            }
        }
        self.core.call_in_session("schedule", json!({}))?;
        self.signal();
        Ok(())
    }

    /// Recover the durable injection evidence, including mid-turn input.
    fn converge(&self, run: &TurnRun, outcome: TurnOutcome) {
        let applied = self
            .runner(&run.agent_id)
            .and_then(|runner| runner.applied_delivery_ids(run))
            .unwrap_or_else(|| run.input_delivery_ids.clone());
        self.offered.lock().unwrap().entry(run.run_id.clone()).or_default().extend(applied);
        self.finalize(run, outcome);
    }

    // -- helpers -------------------------------------------------------------

    /// A single wait observation; interactive callers must be able to report
    /// read failures instead of treating them as more work to wait for.
    pub(crate) fn is_settled(&self) -> Result<bool, String> {
        let state = self.core.state_brief()?;
        let runs: Vec<TurnRun> = serde_json::from_value(state.get("runs").cloned().unwrap_or(Json::Null))
            .map_err(|error| format!("读取回合状态失败：{error}"))?;
        Ok(!runs.iter().any(|run| run.status.is_active())
            && self.inflight.lock().unwrap().is_empty()
            && self.finalizing.lock().unwrap().is_empty())
    }

    /// Wait until no runs are executing or queued.
    pub fn settle(&self, timeout_s: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(timeout_s);
        while Instant::now() < deadline {
            if self.is_settled().unwrap_or(false) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// Status of one agent row (the worker reports this to the UI).
    pub fn agent_status(&self, agent_id: &str) -> Option<AgentStatus> {
        let state = self.core.state_brief().ok()?;
        state
            .get("agents")?
            .as_array()?
            .iter()
            .find(|a| a.get("id").and_then(|v| v.as_str()) == Some(agent_id))
            .and_then(|a| a.get("status").cloned())
            .and_then(|v| serde_json::from_value(v).ok())
    }
}
