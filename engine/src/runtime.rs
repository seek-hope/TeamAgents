//! Session execution loop (runtime.py, asyncio → threads).
//! Every authoritative state change goes through the core; this module only
//! decides *when* a member turn runs, and reports its outcome.

use crate::core_client::CoreClient;
use crate::gateway::{ApprovalGate, ToolGateway};
use serde_json::{json, Value as Json};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};
use teamagents_core::control::TurnOutcome;
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
    fn deliver_mid_turn(&self, run_id: &str, items: Vec<Json>);
    /// Optional restart convergence (RT-04): a backend that survives our
    /// restart reports the live status of a parked turn.
    fn reconcile(&self, _run: &TurnRun) -> Option<TurnStatus> {
        None
    }
    /// Optional: resolve a parked approval in a backend that asked for it.
    fn resolve_approval(&self, _approval_id: &str, _decision: &str) -> bool {
        false
    }
    fn close(&self) {}
}

pub type RunnerFactory = Box<dyn Fn(&AgentSpec) -> Result<Arc<dyn AgentRunner>, String> + Send + Sync>;
pub type ToolExecutor = Arc<dyn Fn(&str, &str, &Json) -> Result<Json, String> + Send + Sync>;

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

type Sink = Box<dyn Fn(&str, &str, &str) + Send + Sync>;

/// Runtime-originated notifications: stream deltas to the UI, external backend
/// status/progress into the event log. Runners hold this, never the Runtime.
pub struct Notify {
    core: Arc<CoreClient>,
    stream: Mutex<Option<Sink>>,
    waker: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl Notify {
    pub fn new(core: Arc<CoreClient>) -> Arc<Self> {
        Arc::new(Self { core, stream: Mutex::new(None), waker: Mutex::new(None) })
    }

    pub fn set_stream_sink(&self, sink: Sink) {
        *self.stream.lock().unwrap() = Some(sink);
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
        if text.is_empty() {
            return;
        }
        if let Ok(stream) = self.stream.lock() {
            if let Some(sink) = stream.as_ref() {
                sink(run_id, agent_id, text);
            }
        }
    }

    /// codex.py::_on_status — live status changes from an external backend.
    pub fn note_external_status(&self, run_id: &str, status: TurnStatus) {
        let Ok(state) = self.core.state() else { return };
        let runs: Vec<TurnRun> = serde_json::from_value(state.get("runs").cloned().unwrap_or(Json::Null)).unwrap_or_default();
        let Some(run) = runs.iter().find(|r| r.run_id == run_id) else { return };
        if run.status.is_terminal() {
            return;
        }
        let agent_id = run.agent_id.clone();
        core_best_effort(&self.core, "run status update", "set_run_status", json!({"run_id": run_id, "status": status}));
        if status == TurnStatus::WaitingApproval {
            let pending: Vec<Json> = state
                .get("pending_approvals")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if let Some(approval) = pending
                .iter()
                .filter(|a| a.get("run_id").and_then(|v| v.as_str()) == Some(run_id))
                .next_back()
            {
                core_best_effort(&self.core, "approval_requested event", "emit", json!({
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
                }));
            }
        }
        self.wake();
    }

    /// codex.py::_on_progress — a human-readable progress line from a backend.
    pub fn note_external_progress(&self, run_id: &str, text: &str) {
        if text.is_empty() {
            return;
        }
        let Ok(state) = self.core.state() else { return };
        let runs: Vec<TurnRun> = serde_json::from_value(state.get("runs").cloned().unwrap_or(Json::Null)).unwrap_or_default();
        let Some(run) = runs.iter().find(|r| r.run_id == run_id) else { return };
        let requester = run.task_id.as_ref().and_then(|task_id| {
            state
                .get("tasks")
                .and_then(|v| v.as_array())
                .and_then(|tasks| tasks.iter().find(|t| t.get("task_id").and_then(|v| v.as_str()) == Some(task_id.as_str())))
                .and_then(|task| task.get("requester"))
                .cloned()
        });
        core_best_effort(&self.core, "run_progress event", "emit", json!({
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
        }));
        self.wake();
    }
}

struct RunSlot {
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    cancel_started: AtomicBool,
}

pub struct Runtime {
    pub core: Arc<CoreClient>,
    pub notify: Arc<Notify>,
    runners: Mutex<HashMap<String, Arc<dyn AgentRunner>>>,
    factory: Option<RunnerFactory>,
    inflight: Mutex<HashMap<String, Arc<RunSlot>>>,
    approvals: Arc<ApprovalGate>,
    executor: ToolExecutor,
    offered: Mutex<HashMap<String, HashSet<i64>>>,
    steps: Arc<Mutex<HashMap<String, i64>>>,
    limits: RuntimeLimits,
    closed: AtomicBool,
    wake: (Mutex<bool>, Condvar),
    loop_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    self_ref: Mutex<Weak<Runtime>>,
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
            approvals,
            executor,
            offered: Mutex::new(HashMap::new()),
            steps: Arc::new(Mutex::new(HashMap::new())),
            limits,
            closed: AtomicBool::new(false),
            wake: (Mutex::new(false), Condvar::new()),
            loop_thread: Mutex::new(None),
            self_ref: Mutex::new(Weak::new()),
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

    pub fn add_runner(&self, agent_id: &str, runner: Arc<dyn AgentRunner>) {
        self.runners.lock().unwrap().insert(agent_id.to_string(), runner);
    }

    pub fn runner(&self, agent_id: &str) -> Option<Arc<dyn AgentRunner>> {
        self.runners.lock().unwrap().get(agent_id).cloned()
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

    /// Stop scheduling and release member backends.
    ///
    /// ponytail: in-flight turns are NOT waited for. A member mid-HTTP-call
    /// cannot be aborted here, and blocking quit/switch on it made closing a
    /// session wait for the whole model call. The turn stays RUNNING in the DB
    /// and the next start reconciles it (RT-04) — the same recovery path a
    /// killed process uses.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.signal();
        if let Some(handle) = self.loop_thread.lock().unwrap().take() {
            let _ = handle.join();
        }
        let runners: Vec<Arc<dyn AgentRunner>> = self.runners.lock().unwrap().values().cloned().collect();
        for runner in runners {
            runner.close();
        }
    }

    fn run_loop(&self) {
        while !self.closed.load(Ordering::SeqCst) {
            self.start_ready_runs();
            self.sleep_or_wake(50);
        }
    }

    // -- ingress -------------------------------------------------------------

    pub fn submit(&self, action: TeamAction) -> Result<Receipt, String> {
        let receipt = self.core.submit(&action)?;
        if action.kind == teamagents_core::models::ActionKind::ApprovalDecision && receipt.ok {
            let payload = &action.payload;
            let approval_id = payload.get("approval_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let decision = payload.get("decision").and_then(|v| v.as_str()).unwrap_or("").to_string();
            self.deliver_approval_decision(&approval_id, &decision);
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
        let Ok(state) = self.core.state() else { return };
        let runs: Vec<TurnRun> = serde_json::from_value(state.get("runs").cloned().unwrap_or(Json::Null)).unwrap_or_default();
        let Some(run) = runs.iter().find(|r| r.run_id == run_id) else { return };
        if let Some(runner) = self.runner(&run.agent_id) {
            // false = the backend has no waiter for it (the wait timed out and
            // the approval was already voided, or the run moved on). At least
            // leave a trace instead of dropping the user's decision silently.
            if !runner.resolve_approval(approval_id, decision) {
                eprintln!(
                    "approval {approval_id} decided as {decision} but member {} has no waiter for it",
                    run.agent_id
                );
            }
        }
    }

    fn drain_mid_turn(&self) {
        let Ok(reply) = self.core.call_in_session("drain_mid_turn", json!({})) else { return };
        let pushes = reply.get("pushes").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        for push in pushes {
            let run_id = push.get("run_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let items = push.get("items").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            let Ok(state) = self.core.state() else { return };
            let runs: Vec<TurnRun> = serde_json::from_value(state.get("runs").cloned().unwrap_or(Json::Null)).unwrap_or_default();
            let Some(run) = runs.iter().find(|r| r.run_id == run_id) else { continue };
            if let Some(runner) = self.runner(&run.agent_id) {
                runner.deliver_mid_turn(&run_id, items);
                let mut offered = self.offered.lock().unwrap();
                let slot = offered.entry(run_id.clone()).or_default();
                for id in &run.input_delivery_ids {
                    slot.insert(*id);
                }
            }
        }
    }

    pub fn state(&self) -> Result<Json, String> {
        self.core.state()
    }

    // -- scheduler -----------------------------------------------------------

    fn start_ready_runs(&self) {
        let Ok(state) = self.core.state() else { return };
        let session = state.get("session").cloned().unwrap_or(Json::Null);
        if session.is_null() {
            return;
        }
        if session.get("status").and_then(|v| v.as_str()) == Some("PAUSED") {
            return;
        }
        let leader_id = state.get("leader_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let limit_workers = state
            .get("limits")
            .and_then(|l| l.get("max_parallel_workers"))
            .and_then(|v| v.as_i64())
            .unwrap_or(self.limits.max_parallel_workers);
        let runs: Vec<TurnRun> = serde_json::from_value(state.get("runs").cloned().unwrap_or(Json::Null)).unwrap_or_default();
        let removed: HashSet<String> = state
            .get("agents")
            .and_then(|v| v.as_array())
            .map(|agents| {
                agents
                    .iter()
                    .filter(|a| a.get("status").and_then(|v| v.as_str()) == Some("REMOVED"))
                    .filter_map(|a| a.get("id").and_then(|v| v.as_str()).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

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
            if self.inflight.lock().unwrap().contains_key(&run.run_id) {
                continue;
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
            if removed.contains(&run.agent_id) {
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
        let state = self.core.state().ok()?;
        let spec = state.get("spec")?.clone();
        let agents: Vec<AgentSpec> = serde_json::from_value(spec.get("agents")?.clone()).ok()?;
        agents.into_iter().find(|a| a.id == agent_id)
    }

    fn spawn_execute(&self, run: TurnRun, _runner: Arc<dyn AgentRunner>) {
        let slot = Arc::new(RunSlot { handle: Mutex::new(None), cancel_started: AtomicBool::new(false) });
        self.inflight.lock().unwrap().insert(run.run_id.clone(), slot.clone());
        let Some(runtime) = self.me() else { return };
        let run_id = run.run_id.clone();
        let handle = std::thread::spawn(move || {
            runtime.execute(&run);
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
            core_best_effort(&self.core, "stop timeout bookkeeping", "stop_timeout", json!({"run_id": run.run_id}));
        }
        self.signal();
    }

    // -- executor ------------------------------------------------------------

    fn execute(&self, run: &TurnRun) {
        let guard = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.execute_inner(run)));
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

    fn execute_inner(&self, run: &TurnRun) -> Result<(), String> {
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
        let begin = self.core.call_in_session("begin_run", json!({"run_id": run.run_id}))?;
        let fresh: TurnRun = serde_json::from_value(begin.get("run").cloned().unwrap_or(Json::Null))
            .map_err(|e| format!("bad run: {e}"))?;
        let wake = begin.get("wake").cloned().unwrap_or(Json::Null);
        let view = self.core.call_in_session("agent_view", json!({"agent_id": fresh.agent_id}))?;
        let delivery_ids: Vec<i64> = view
            .get("delivery_ids")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        if !delivery_ids.is_empty() {
            let mut offered = self.offered.lock().unwrap();
            let slot = offered.entry(fresh.run_id.clone()).or_default();
            for id in delivery_ids {
                slot.insert(id);
            }
        }
        let limits = self.effective_limits();
        let timeout = Duration::from_secs(limits.turn_active_timeout_s.max(1) as u64);
        let gateway = ToolGateway::new(
            self.core.clone(),
            &fresh.agent_id,
            &fresh.run_id,
            self.approvals.clone(),
            Some(self.guarded_executor(&fresh.agent_id, &fresh.run_id, limits.max_model_steps_per_turn)),
        );
        let mut outcome = self.run_with_timeout(runner.clone(), &fresh, &view, gateway, &wake, timeout);

        let state = self.core.state()?;
        let runs: Vec<TurnRun> = serde_json::from_value(state.get("runs").cloned().unwrap_or(Json::Null)).unwrap_or_default();
        let cancel_requested = runs
            .iter()
            .find(|r| r.run_id == fresh.run_id)
            .map(|r| r.cancel_requested)
            .unwrap_or(false);
        if cancel_requested
            && matches!(
                outcome.status,
                TurnStatus::Completed | TurnStatus::WaitingTask | TurnStatus::WaitingApproval
            )
        {
            let confirmed = self.interrupt_confirm(&fresh.run_id, runner);
            outcome = TurnOutcome {
                status: confirmed,
                error: None,
                note: Some("cancelled by request".into()),
                reply_text: outcome.reply_text,
            };
        }
        self.finalize(&fresh, outcome);
        Ok(())
    }

    /// Run the member on its own thread so the active-time limit can fire.
    /// On timeout the member is interrupted (Python's `asyncio.wait_for`
    /// cancels the coroutine): without it the thread keeps calling the model
    /// and writing team state after the run is already FAILED.
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
        std::thread::spawn(move || {
            let outcome = runner.start_or_resume(&run_clone, &view_clone, &gateway, &wake_clone);
            let _ = tx.send(outcome);
        });
        match rx.recv_timeout(timeout) {
            Ok(outcome) => outcome,
            Err(RecvTimeoutError::Timeout) => {
                // the interrupt itself must not block the timeout path
                let run_id = run.run_id.clone();
                std::thread::spawn(move || {
                    runner_for_interrupt.request_interrupt(&run_id);
                });
                TurnOutcome {
                    status: TurnStatus::Failed,
                    error: Some(format!("turn active-time limit {}s reached", timeout.as_secs())),
                    note: None,
                    reply_text: None,
                }
            }
            // the member thread panicked before sending: never report that as a
            // timeout (a 1200s claim for an immediate crash sends debugging the
            // wrong way)
            Err(RecvTimeoutError::Disconnected) => TurnOutcome {
                status: TurnStatus::Failed,
                error: Some("member runner crashed before reporting an outcome".into()),
                note: None,
                reply_text: None,
            },
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
    ) -> Arc<dyn Fn(&str, &Json) -> Result<Json, String> + Send + Sync> {
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
            base(&agent_id, tool, args)
        })
    }

    fn effective_limits(&self) -> RuntimeLimits {
        let Ok(state) = self.core.state() else { return self.limits.clone() };
        let field = |name: &str, fallback: i64| -> i64 {
            state
                .get("limits")
                .and_then(|l| l.get(name))
                .and_then(|v| v.as_i64())
                .unwrap_or(fallback)
        };
        RuntimeLimits {
            turn_active_timeout_s: field("turn_active_timeout_s", self.limits.turn_active_timeout_s),
            cancel_confirm_timeout_s: field("cancel_confirm_timeout_s", self.limits.cancel_confirm_timeout_s),
            max_model_steps_per_turn: field("max_model_steps_per_turn", self.limits.max_model_steps_per_turn),
            max_parallel_workers: field("max_parallel_workers", self.limits.max_parallel_workers),
        }
    }

    fn finalize(&self, run: &TurnRun, outcome: TurnOutcome) {
        let ack: Vec<i64> = self
            .offered
            .lock()
            .unwrap()
            .remove(&run.run_id)
            .map(|ids| ids.into_iter().collect())
            .unwrap_or_default();
        core_best_effort(&self.core, "run finalization", "finalize_run", json!({
            "run_id": run.run_id,
            "status": outcome.status,
            "error": outcome.error,
            "note": outcome.note,
            "reply_text": outcome.reply_text,
            "ack_ids": ack,
        }));
        self.drain_mid_turn();
        self.signal();
    }

    // -- restart convergence (RT-04) ------------------------------------------

    /// After a restart: re-check in-flight runs, never blind-retry side effects.
    pub fn reconcile(&self) {
        let Ok(state) = self.core.state() else { return };
        let runs: Vec<TurnRun> = serde_json::from_value(state.get("runs").cloned().unwrap_or(Json::Null)).unwrap_or_default();
        let parked: Vec<TurnRun> = runs
            .iter()
            .filter(|r| matches!(r.status, TurnStatus::Running | TurnStatus::WaitingTask | TurnStatus::WaitingApproval))
            .cloned()
            .collect();
        for run in parked {
            let runner = self.runner(&run.agent_id);
            let mut status = runner.as_ref().and_then(|r| r.query_state(&run.run_id));
            if status.is_none() {
                if let Some(runner) = &runner {
                    status = runner.reconcile(&run);
                }
            }
            if run.status != TurnStatus::Running {
                if status == Some(run.status) {
                    continue;
                }
                match status {
                    Some(s) if s.is_terminal() => self.converge(&run, s, None),
                    Some(s) => self.finalize(
                        &run,
                        TurnOutcome { status: s, error: None, note: Some("pause restored".into()), reply_text: None },
                    ),
                    None => self.converge(
                        &run,
                        TurnStatus::OutcomeUnknown,
                        Some("suspended turn could not be restored after restart"),
                    ),
                }
                continue;
            }
            if let Some(s) = status {
                if s.is_terminal() || matches!(s, TurnStatus::WaitingTask | TurnStatus::WaitingApproval) {
                    self.converge(&run, s, None);
                }
                continue;
            }
            if run.external_turn_id.is_some() {
                self.converge(
                    &run,
                    TurnStatus::OutcomeUnknown,
                    Some("external turn outcome could not be confirmed"),
                );
            } else {
                // in-process runner only: safe to re-run the segment
                core_best_effort(&self.core, "run requeue", "requeue_run", json!({"run_id": run.run_id}));
            }
        }
        core_best_effort(&self.core, "scheduling pass", "schedule", json!({}));
        self.signal();
    }

    /// Finalize a run found at restart; its input counts as injected (F-C3/RT-05).
    fn converge(&self, run: &TurnRun, status: TurnStatus, error: Option<&str>) {
        self.offered
            .lock()
            .unwrap()
            .entry(run.run_id.clone())
            .or_default()
            .extend(run.input_delivery_ids.iter().copied());
        self.finalize(
            run,
            TurnOutcome { status, error: error.map(str::to_string), note: None, reply_text: None },
        );
    }

    // -- helpers -------------------------------------------------------------

    /// Wait until no runs are executing or queued.
    pub fn settle(&self, timeout_s: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(timeout_s);
        while Instant::now() < deadline {
            let busy = self
                .core
                .state()
                .ok()
                .and_then(|state| {
                    serde_json::from_value::<Vec<TurnRun>>(state.get("runs").cloned().unwrap_or(Json::Null)).ok()
                })
                .map(|runs| runs.iter().any(|r| r.status.is_active()))
                .unwrap_or(true);
            if !busy && self.inflight.lock().unwrap().is_empty() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// Status of one agent row (the worker reports this to the UI).
    pub fn agent_status(&self, agent_id: &str) -> Option<AgentStatus> {
        let state = self.core.state().ok()?;
        state
            .get("agents")?
            .as_array()?
            .iter()
            .find(|a| a.get("id").and_then(|v| v.as_str()) == Some(agent_id))
            .and_then(|a| a.get("status").cloned())
            .and_then(|v| serde_json::from_value(v).ok())
    }
}
