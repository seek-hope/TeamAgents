"""Session runtime: member execution handles, concurrency, turn lifecycle.

The runtime consumes persisted execution intents (QUEUED TurnRuns) created by
the control flow, starts each member's own execution, and writes results back
through one serialized path. It never waits for all members before reacting;
one member's crash or pause changes only that member's execution state.
"""

from __future__ import annotations

import asyncio
import contextlib
import logging
from pathlib import Path
from typing import Any, Callable

from .agents import AgentRunner, FakeMember, TEAM_TOOLS, ToolGateway, TurnOutcome, WakeInfo
from .control import Control, EventDraft
from .models import (
    ActionKind,
    AgentStatus,
    ApprovalStatus,
    EventKind,
    Receipt,
    SessionStatus,
    TaskStatus,
    TeamAction,
    TurnRun,
    TurnStatus,
    TURN_TERMINAL_STATUSES,
    UserConfig,
    new_id,
)
from .permissions import ApprovalGate, PermissionPolicy
from .storage import Store
from .views import build_agent_view

log = logging.getLogger("teamagents.runtime")


class SessionRuntime:
    """Owns execution for one session. Single writer, single executor loop."""

    def __init__(self, store: Store, session_id: str,
                 catalog: UserConfig | None = None,
                 runners: dict[str, AgentRunner] | None = None,
                 approvals: ApprovalGate | None = None,
                 tool_executor: Callable[[str, dict[str, Any]], Any] | None = None,
                 lock: asyncio.Lock | None = None,
                 cleanup: Callable[[], Any] | None = None,
                 runner_factory: Callable[[Any], AgentRunner] | None = None):
        self.store = store
        self.session_id = session_id
        self.catalog = catalog or UserConfig()
        self.runners: dict[str, AgentRunner] = runners or {}
        self.approvals = approvals or ApprovalGate(
            store, session_id, PermissionPolicy())
        self.tool_executor = tool_executor
        self.lock = lock or asyncio.Lock()
        self.cleanup = cleanup
        self.runner_factory = runner_factory
        #: UI sinks; high-frequency deltas are coalesced by the consumer, never
        #: persisted (plan section 13: bounded queue, coalesced refresh)
        self.stream_sink: Callable[[str, str, str], None] | None = None
        self.control = Control(store, session_id, self.catalog)
        self._inflight: dict[str, asyncio.Task] = {}
        self._cancel_tasks: dict[str, asyncio.Task] = {}
        self._wake = asyncio.Event()
        self._closed = False
        self._loop_task: asyncio.Task | None = None
        self._step_counts: dict[str, int] = {}
        self._runner_revisions: dict[str, int] = {}
        #: deliveries handed to a runner as part of a segment view (injected into
        #: the turn's context); only these may be acknowledged when the run ends
        self._offered: dict[str, set[int]] = {}

    # ------------------------------------------------------------- lifecycle

    def add_runner(self, agent_id: str, runner: AgentRunner) -> None:
        self.runners[agent_id] = runner

    def _refresh_runner_if_stale(self, agent_id: str, spec) -> bool:
        """Rebuild a member's backend when its config revision moved (model change,
        tool change...). Returns True when a rebuild was scheduled (skip this run
        until the swap completes)."""
        if self.runner_factory is None:
            return False
        revision = self.store.agent_config_revision(self.session_id, agent_id)
        known = self._runner_revisions.get(agent_id)
        if known is None:
            self._runner_revisions[agent_id] = revision
            return False
        if known == revision:
            return False
        try:
            new_runner = self.runner_factory(spec.agent(agent_id))
        except Exception as e:
            self.store.set_agent_status(self.session_id, agent_id, AgentStatus.IDLE)
            self._emit([EventDraft(kind=EventKind.MEMBER_STATUS,
                                   payload={"agent_id": agent_id,
                                            "status": "REBUILD_FAILED",
                                            "error": str(e)})],
                       actor_id="system")
            self._runner_revisions[agent_id] = revision
            return False
        old = self.runners.get(agent_id)
        self.runners[agent_id] = new_runner
        self._runner_revisions[agent_id] = revision
        if hasattr(new_runner, "status_hook"):
            new_runner.status_hook = self.note_external_status
        if hasattr(new_runner, "progress_hook"):
            new_runner.progress_hook = self.note_external_progress
        if hasattr(new_runner, "stream_hook"):
            new_runner.stream_hook = self.note_stream_chunk
        close = getattr(old, "aclose", None)
        if close is not None:
            asyncio.create_task(close())
        self._wake.set()
        return True

    async def start(self) -> None:
        self._closed = False
        await self.reconcile()
        self._loop_task = asyncio.create_task(self._loop(), name=f"ta-loop-{self.session_id}")

    async def close(self) -> None:
        self._closed = True
        self._wake.set()
        for task in list(self._inflight.values()):
            task.cancel()
        if self._inflight:
            await asyncio.gather(*self._inflight.values(), return_exceptions=True)
        if self._loop_task:
            with contextlib.suppress(asyncio.CancelledError):
                await self._loop_task
        if self.cleanup is not None:
            result = self.cleanup()
            if asyncio.iscoroutine(result):
                await result

    async def reconcile(self) -> None:
        """After a restart: re-check runs that were in flight, never blind-retry side effects."""
        runs = self.store.runs_for_session(
            self.session_id,
            [TurnStatus.RUNNING, TurnStatus.WAITING_TASK, TurnStatus.WAITING_APPROVAL])
        for run in runs:
            runner = self.runners.get(run.agent_id)
            state = runner.query_state(run.run_id) if runner is not None else None
            if state is None and runner is not None and hasattr(runner, "reconcile"):
                # this also restores the runner-side pause marker for resumable turns
                state = await runner.reconcile(run)
            if run.status is not TurnStatus.RUNNING:
                # a parked turn (RT-04/AD-7): only the runner's own checkpoint can
                # make it resumable; anything else converges instead of replaying
                # the whole input as if it were a new message
                if state == run.status:
                    continue
                if state is None or state in TURN_TERMINAL_STATUSES:
                    self._converge(run, TurnOutcome(
                        status=state if state is not None else TurnStatus.OUTCOME_UNKNOWN,
                        error=None if state is not None else
                        "suspended turn could not be restored after restart"))
                else:
                    # the checkpoint disagrees with the row: adopt its pause
                    self._finalize(run, TurnOutcome(status=state, note="pause restored"))
                continue
            if state is not None:
                if state in TURN_TERMINAL_STATUSES or state in (
                        TurnStatus.WAITING_TASK, TurnStatus.WAITING_APPROVAL):
                    self._converge(run, TurnOutcome(status=state))
                else:
                    self.store.set_run_status(run.run_id, state)
                continue
            if run.external_turn_id:
                self._converge(run, TurnOutcome(
                    status=TurnStatus.OUTCOME_UNKNOWN,
                    error="external turn outcome could not be confirmed"))
            else:
                # in-process runner only: safe to re-run the segment
                self.store.update_run_status_where(
                    run.run_id, TurnStatus.RUNNING, TurnStatus.QUEUED)
        # leftover parked turns only wake when scheduling re-evaluates them:
        # waited-on tasks may have finished while the process was down
        self.control.schedule()
        self._wake.set()

    def _converge(self, run: TurnRun, outcome: TurnOutcome) -> None:
        """Finalize a run found at restart through the normal convergence path.

        Its input was handed to a runner before the process died, so it counts as
        injected: the run ends without re-delivering (F-C3/RT-05) and without a
        blind retry of external side effects.
        """
        self._offered[run.run_id] = set(run.input_delivery_ids)
        self._finalize(run, outcome)

    # --------------------------------------------------------------- ingress

    def submit(self, action: TeamAction) -> Receipt:
        """Synchronous submit through the single serialized control entry."""
        receipt = self.control.submit(action)
        if action.kind is ActionKind.APPROVAL_DECISION and receipt.ok:
            self._deliver_approval_decision(action.payload["approval_id"],
                                            action.payload["decision"])
        self._drain_mid_turn()
        self._wake.set()
        return receipt

    def _deliver_approval_decision(self, approval_id: str, decision: str) -> None:
        """External backends (Codex) hold a live turn parked on the decision."""
        approval = self.store.get_approval(approval_id)
        if approval is None:
            return
        run = self.store.get_run(approval.run_id)
        runner = self.runners.get(run.agent_id) if run else None
        resolve = getattr(runner, "resolve_approval", None)
        if resolve is not None:
            resolve(approval_id, decision)

    def note_external_status(self, run_id: str, status: TurnStatus) -> None:
        """A backend (Codex) reports live status changes for a parked turn."""
        run = self.store.get_run(run_id)
        if run is None or run.status in (TurnStatus.COMPLETED, TurnStatus.FAILED,
                                          TurnStatus.CANCELLED, TurnStatus.OUTCOME_UNKNOWN):
            return
        self.store.set_run_status(run_id, status)
        if status is TurnStatus.WAITING_APPROVAL:
            self.store.set_agent_status(self.session_id, run.agent_id, AgentStatus.WAITING)
            approvals = [a for a in self.store.pending_approvals(self.session_id)
                         if a is not None and a.run_id == run_id]
            if approvals:
                self._emit([EventDraft(
                    kind=EventKind.APPROVAL_REQUESTED,
                    payload={"approval_id": approvals[-1].approval_id,
                             "agent_id": run.agent_id, "run_id": run_id,
                             "scope": approvals[-1].requested_scope})],
                    actor_id=run.agent_id)
        elif status is TurnStatus.RUNNING:
            self.store.set_agent_status(self.session_id, run.agent_id, AgentStatus.BUSY)
        self._wake.set()

    def note_external_progress(self, run_id: str, text: str) -> None:
        """A backend reports structured progress; it becomes a team event."""
        run = self.store.get_run(run_id)
        if run is None or not text:
            return
        task = self.store.get_task(run.task_id) if run.task_id else None
        self._emit([EventDraft(kind=EventKind.RUN_PROGRESS,
                               payload={"run_id": run_id, "agent_id": run.agent_id,
                                        "text": text[:2000],
                                        "requester": task.requester if task else None,
                                        "task_id": run.task_id})],
                   actor_id=run.agent_id)

    def note_stream_chunk(self, run_id: str, agent_id: str, text: str) -> None:
        """Transient model output for the UI (not persisted)."""
        if self.stream_sink is not None and text:
            self.stream_sink(run_id, agent_id, text)

    def set_stream_sink(self, sink: Callable[[str, str, str], None] | None) -> None:
        self.stream_sink = sink

    async def submit_async(self, action: TeamAction) -> Receipt:
        async with self.lock:
            return self.submit(action)

    def user_message(self, text: str, action_id: str | None = None,
                     supplement: bool = False) -> Receipt:
        return self.submit(TeamAction(
            action_id=action_id or new_id("user"),
            session_id=self.session_id,
            actor_id="user",
            kind=ActionKind.USER_SUPPLEMENT if supplement else ActionKind.USER_MESSAGE,
            payload={"text": text},
        ))

    def _drain_mid_turn(self) -> None:
        pushes = getattr(self.control, "mid_turn_pushes", [])
        while pushes:
            run_id, items = pushes.pop(0)
            runner = None
            run = self.store.get_run(run_id)
            if run is not None:
                runner = self.runners.get(run.agent_id)
            if runner is not None:
                runner.deliver_mid_turn(run_id, items)
                if run is not None:
                    # a mid-turn push is injection too: these inputs are part of
                    # the run and must be acknowledged when it ends (RT-05/F-C3)
                    self._offered[run_id] = (self._offered.get(run_id, set())
                                             | set(run.input_delivery_ids))

    # ---------------------------------------------------------------- executor

    async def _loop(self) -> None:
        while not self._closed:
            self._start_ready_runs()
            if self._inflight:
                done, _ = await asyncio.wait(
                    list(self._inflight.values()) + [asyncio.create_task(self._wake.wait())],
                    return_when=asyncio.FIRST_COMPLETED)
                self._wake.clear()
            else:
                await self._wake.wait()
                self._wake.clear()

    def _start_ready_runs(self) -> None:
        session = self.store.get_session(self.session_id)
        if session is None or SessionStatus(session["status"]) is SessionStatus.PAUSED:
            return
        spec = self.store.load_team_spec(self.session_id)
        leaders_busy = sum(1 for r in self._inflight.values() if not r.done())
        workers_busy = 0
        for run_id in self._inflight:
            run = self.store.get_run(run_id)
            # a turn parked on an approval or a wait does not hold an execution slot
            if (run is not None and run.agent_id != spec.leader_id
                    and run.status in (TurnStatus.QUEUED, TurnStatus.RUNNING)):
                workers_busy += 1
        for run in self.store.runs_for_session(self.session_id,
                                               [TurnStatus.QUEUED, TurnStatus.RUNNING]):
            if run.run_id in self._inflight:
                continue
            if any(a.id == run.agent_id for a in spec.agents):
                if self._refresh_runner_if_stale(run.agent_id, spec):
                    continue  # a fresh backend is ready on the next pass
            if run.agent_id in self.runners:
                if run.agent_id == spec.leader_id:
                    pass  # the Leader keeps its own slot
                elif workers_busy >= spec.limits.max_parallel_workers:
                    continue
            elif self.runner_factory is not None:
                # members added by a topology patch get their backend on demand
                try:
                    self.runners[run.agent_id] = self.runner_factory(
                        spec.agent(run.agent_id))
                except Exception as e:
                    self._finalize(run, TurnOutcome(
                        status=TurnStatus.FAILED,
                        error=f"cannot start member {run.agent_id!r}: {e}"))
                    continue
            else:
                continue
            status = self.store.agent_status(self.session_id, run.agent_id)
            if status is AgentStatus.REMOVED:
                continue
            task = asyncio.create_task(self._execute(run), name=f"run-{run.run_id}")
            self._inflight[run.run_id] = task
            if run.agent_id != spec.leader_id:
                workers_busy += 1
            leaders_busy += 1
            _ = leaders_busy
        self._watch_cancellations()

    def _watch_cancellations(self) -> None:
        """Ask the owning backend to stop runs the user/Leader cancelled."""
        for run_id, task in list(self._inflight.items()):
            if task.done() or run_id in self._cancel_tasks:
                continue
            if not self.store.run_cancel_requested(run_id):
                continue
            run = self.store.get_run(run_id)
            runner = self.runners.get(run.agent_id) if run else None
            if runner is None:
                continue
            self._cancel_tasks[run_id] = asyncio.create_task(
                self._request_stop(run, runner), name=f"cancel-{run_id}")

    async def _request_stop(self, run: TurnRun, runner: AgentRunner) -> None:
        """Cancellation waits for the backend to confirm the stop, or marks
        OUTCOME_UNKNOWN once the confirmation timeout expires."""
        spec = self.store.load_team_spec(self.session_id)
        try:
            await asyncio.wait_for(runner.request_interrupt(run.run_id),
                                   timeout=spec.limits.cancel_confirm_timeout_s)
        except asyncio.TimeoutError:
            self.store.update_run_status_where(
                run.run_id, TurnStatus.RUNNING, TurnStatus.OUTCOME_UNKNOWN)
            expired = self.control.expire_run_approvals(run.run_id)
            if expired:
                self._emit(expired, actor_id=run.agent_id)
        finally:
            self._cancel_tasks.pop(run.run_id, None)
            self._wake.set()

    async def _execute(self, run: TurnRun) -> None:
        try:
            await self._execute_inner(run)
        except asyncio.CancelledError:
            raise
        except Exception as e:  # a member crash must not take the team down
            log.exception("run %s crashed", run.run_id)
            self._finalize(run, TurnOutcome(status=TurnStatus.FAILED,
                                            error=f"{type(e).__name__}: {e}"))
        finally:
            self._inflight.pop(run.run_id, None)
            self._cancel_tasks.pop(run.run_id, None)
            self._offered.pop(run.run_id, None)
            self._wake.set()

    async def _execute_inner(self, run: TurnRun) -> None:
        spec = self.store.load_team_spec(self.session_id)
        runner = self.runners.get(run.agent_id)
        if runner is None:
            self._finalize(run, TurnOutcome(status=TurnStatus.FAILED,
                                            error=f"no runner for member {run.agent_id}"))
            return
        if run.status is TurnStatus.QUEUED:
            self.store.set_run_status(run.run_id, TurnStatus.RUNNING)
        run = self.store.get_run(run.run_id) or run
        self.store.set_agent_status(self.session_id, run.agent_id, AgentStatus.BUSY)
        wake = self._wake_info(run)
        if run.task_id:
            task = self.store.get_task(run.task_id)
            if task is not None and self.store.compare_and_set_task(
                    run.task_id, TaskStatus.PENDING, TaskStatus.RUNNING):
                self._emit([EventDraft(kind=EventKind.TASK_STARTED,
                                       payload={"task_id": task.task_id,
                                                "assignee": task.assignee,
                                                "requester": task.requester,
                                                "status": "RUNNING"},
                                       task_id=task.task_id)],
                           actor_id=run.agent_id)
        self._emit([EventDraft(kind=EventKind.RUN_STARTED,
                               payload={"run_id": run.run_id, "agent_id": run.agent_id,
                                        "status": "RUNNING", "wake": wake.reason})],
                   actor_id=run.agent_id)

        gateway = ToolGateway(self.control, self.session_id, run.agent_id, run.run_id,
                              self.approvals, self._guarded_executor(run),
                              submit=self.submit)
        view = build_agent_view(self.store, spec, self.session_id, run.agent_id, run)
        if view.delivery_ids:
            # hand-off record: these deliveries are now part of the turn's context
            # (a later segment may offer more; the union is acked at the end)
            self._offered[run.run_id] = (
                self._offered.get(run.run_id, set()) | set(view.delivery_ids))
        timeout = spec.limits.turn_active_timeout_s
        try:
            outcome = await asyncio.wait_for(
                runner.start_or_resume(run, view, gateway, wake), timeout=timeout)
        except asyncio.TimeoutError:
            outcome = TurnOutcome(status=TurnStatus.FAILED,
                                  error=f"turn active-time limit {timeout}s reached")

        if self.store.run_cancel_requested(run.run_id) and outcome.status in (
                TurnStatus.COMPLETED, TurnStatus.WAITING_TASK, TurnStatus.WAITING_APPROVAL):
            spec = self.store.load_team_spec(self.session_id)
            try:
                confirmed = await asyncio.wait_for(
                    runner.request_interrupt(run.run_id),
                    timeout=spec.limits.cancel_confirm_timeout_s)
            except asyncio.TimeoutError:
                confirmed = TurnStatus.OUTCOME_UNKNOWN
            outcome = TurnOutcome(status=confirmed, note="cancelled by request")
        self._finalize(run, outcome)

    def _guarded_executor(self, run: TurnRun) -> Callable[[str, dict[str, Any]], Any]:
        def execute(tool_name: str, args: dict[str, Any]) -> Any:
            spec = self.store.load_team_spec(self.session_id)
            used = self._step_counts.get(run.run_id, 0) + 1
            self._step_counts[run.run_id] = used
            if used > spec.limits.max_model_steps_per_turn:
                raise RuntimeError(
                    f"step limit {spec.limits.max_model_steps_per_turn} reached for this turn")
            if self.tool_executor is None:
                raise RuntimeError(f"no tool executor configured for {tool_name!r}")
            return self.tool_executor(tool_name, args)
        return execute

    def _wake_info(self, run: TurnRun) -> WakeInfo:
        decisions = self.store.decided_approvals_for_run(run.run_id)
        if decisions:
            return WakeInfo(reason="approval",
                            payload={"decisions": [{"approval_id": d.approval_id,
                                                    "status": d.status} for d in decisions],
                                     "denied": any(d.status is ApprovalStatus.DENIED
                                                   for d in decisions)})
        kinds = self.store.delivery_event_kinds(run.input_delivery_ids)
        if kinds and kinds[-1] == EventKind.USER_MESSAGE:
            return WakeInfo(reason="user_input", payload={"kinds": kinds})
        if run.waiting_on:
            results = {tid: self._task_snapshot(tid) for tid in run.waiting_on}
            return WakeInfo(reason="task_results",
                            payload={"task_ids": run.waiting_on, "results": results})
        return WakeInfo(reason="new_input", payload={})

    def _task_snapshot(self, task_id: str) -> dict[str, Any]:
        task = self.store.get_task(task_id)
        if task is None:
            return {"task_id": task_id, "status": "UNKNOWN"}
        return {"task_id": task.task_id, "status": task.status,
                "result_refs": task.result_refs}

    # -------------------------------------------------------------- finalize

    def _finalize(self, run: TurnRun, outcome: TurnOutcome) -> None:
        """Apply the turn result: completion requests, task semantics, deliveries."""
        events: list[EventDraft] = []
        req = self.store.completion_request(run.run_id)
        terminal = outcome.status in (TurnStatus.COMPLETED, TurnStatus.FAILED,
                                      TurnStatus.CANCELLED, TurnStatus.OUTCOME_UNKNOWN)
        with self.store.tx():
            if terminal:
                # a turn that ended can never use a pending decision (RT-06)
                events.extend(self.control.expire_run_approvals(run.run_id))
            if outcome.status is TurnStatus.COMPLETED and req is not None:
                if req["task_id"]:
                    task = self.store.get_task(req["task_id"])
                    refs = [str(r) for r in __import__("json").loads(req["result_refs"])]
                    if task is not None and self.store.compare_and_set_task(
                            task.task_id, task.status, TaskStatus.SUCCEEDED, result_refs=refs):
                        waiters = self.store.waiters_for_task(self.session_id, task.task_id)
                        events.append(EventDraft(
                            kind=EventKind.TASK_COMPLETED,
                            payload={"task_id": task.task_id, "assignee": task.assignee,
                                     "requester": task.requester, "status": "SUCCEEDED",
                                     "result_refs": refs,
                                     "summary": req["summary"]},
                            task_id=task.task_id,
                            push=sorted(set(waiters) | {task.requester}),
                        ))
                else:
                    self.store.set_goal_state(self.session_id, run.goal_id or "", "done")
                    events.append(EventDraft(
                        kind=EventKind.GOAL_DONE,
                        payload={"goal_id": run.goal_id, "agent_id": run.agent_id,
                                 "summary": req["summary"]},
                        push=[],
                    ))
            if outcome.status is TurnStatus.COMPLETED and run.task_id and (
                    req is None
                    or (req["task_id"] and req["task_id"] != run.task_id)):
                # its own task was left unfinished (no completion request at all,
                # or the turn completed a different one): block it for intervention
                # instead of letting it sit RUNNING forever (stale task state)
                task = self.store.get_task(run.task_id)
                if task is not None and task.status in (TaskStatus.PENDING, TaskStatus.RUNNING):
                    if self.store.compare_and_set_task(task.task_id, task.status,
                                                       TaskStatus.BLOCKED):
                        events.append(EventDraft(
                            kind=EventKind.TASK_BLOCKED,
                            payload={"task_id": task.task_id, "assignee": task.assignee,
                                     "requester": task.requester,
                                     "reason": "turn ended without complete_task or wait_for_tasks"},
                            task_id=task.task_id,
                        ))
            elif outcome.status is TurnStatus.FAILED and run.task_id:
                task = self.store.get_task(run.task_id)
                if task is not None and task.status in (TaskStatus.PENDING, TaskStatus.RUNNING):
                    if self.store.compare_and_set_task(task.task_id, task.status,
                                                       TaskStatus.FAILED):
                        events.append(EventDraft(
                            kind=EventKind.TASK_FAILED,
                            payload={"task_id": task.task_id, "assignee": task.assignee,
                                     "requester": task.requester, "error": outcome.error},
                            task_id=task.task_id,
                        ))
            elif outcome.status is TurnStatus.OUTCOME_UNKNOWN and run.task_id:
                task = self.store.get_task(run.task_id)
                if task is not None and task.status in (TaskStatus.PENDING, TaskStatus.RUNNING):
                    self.store.compare_and_set_task(task.task_id, task.status, TaskStatus.BLOCKED)
                    events.append(EventDraft(
                        kind=EventKind.TASK_BLOCKED,
                        payload={"task_id": task.task_id, "assignee": task.assignee,
                                 "requester": task.requester, "reason": outcome.error
                                 or "external turn outcome could not be confirmed"},
                        task_id=task.task_id))
            elif outcome.status is TurnStatus.CANCELLED and run.task_id:
                task = self.store.get_task(run.task_id)
                if task is not None and task.status in (TaskStatus.PENDING, TaskStatus.RUNNING):
                    if self.store.compare_and_set_task(task.task_id, task.status,
                                                       TaskStatus.CANCELLED):
                        events.append(EventDraft(
                            kind=EventKind.TASK_CANCELLED,
                            payload={"task_id": task.task_id, "assignee": task.assignee,
                                     "requester": task.requester,
                                     "status": "CANCELLED"},
                            task_id=task.task_id,
                        ))

            # the terminal status, the member state and the delivery acknowledgement
            # are one write: a crash can never leave "run ended + input un-acked"
            # (F-C3), which used to re-inject the same input into a fresh run
            self.store.set_run_status(run.run_id, outcome.status)
            if outcome.status in (TurnStatus.WAITING_TASK, TurnStatus.WAITING_APPROVAL):
                self.store.set_agent_status(self.session_id, run.agent_id, AgentStatus.WAITING)
            else:
                self.store.set_agent_status(self.session_id, run.agent_id, AgentStatus.IDLE)
            if terminal:
                self._ack_injected(run)
            self._step_counts.pop(run.run_id, None)

            if outcome.status is TurnStatus.WAITING_APPROVAL:
                pending = [a for a in self.store.pending_approvals(self.session_id)
                           if a is not None and a.run_id == run.run_id]
                for a in pending:
                    events.append(EventDraft(
                        kind=EventKind.APPROVAL_REQUESTED,
                        payload={"approval_id": a.approval_id, "agent_id": a.agent_id,
                                 "run_id": a.run_id, "scope": a.requested_scope},
                    ))

            if terminal:
                if outcome.note == "turn_limit":
                    events.append(EventDraft(
                        kind=EventKind.LIMIT_REACHED,
                        payload={"kind": "max_model_steps_per_turn", "run_id": run.run_id,
                                 "agent_id": run.agent_id, "detail": outcome.error}))
                if (outcome.status is TurnStatus.COMPLETED and outcome.reply_text
                        and run.agent_id == self.store.load_team_spec(self.session_id).leader_id):
                    events.append(EventDraft(kind=EventKind.LEADER_REPLY,
                                             payload={"text": outcome.reply_text,
                                                      "run_id": run.run_id}))
                elif outcome.status is TurnStatus.COMPLETED and outcome.reply_text:
                    events.append(EventDraft(
                        kind=EventKind.RUN_PROGRESS,
                        payload={"run_id": run.run_id, "agent_id": run.agent_id,
                                 "text": outcome.reply_text[:2000], "final": True,
                                 "task_id": run.task_id}))
                events.append(EventDraft(
                    kind={TurnStatus.COMPLETED: EventKind.RUN_COMPLETED,
                          TurnStatus.FAILED: EventKind.RUN_FAILED,
                          TurnStatus.CANCELLED: EventKind.RUN_CANCELLED,
                          TurnStatus.OUTCOME_UNKNOWN: EventKind.RUN_FAILED}[outcome.status],
                    payload={"run_id": run.run_id, "agent_id": run.agent_id,
                             "status": outcome.status, "error": outcome.error},
                ))
            if events:
                self._emit(events, actor_id=run.agent_id)
            self.control.schedule()

        if terminal:
            # drop the hand-off ledger only after the acknowledgement committed:
            # a rolled-back finalize must retry with the same evidence (F-C3)
            self._offered.pop(run.run_id, None)

        self._drain_mid_turn()

    def _ack_injected(self, run: TurnRun) -> None:
        """Ack exactly the deliveries a runner was handed for injection (RT-05).

        The ledger is filled in `_execute_inner` when a segment view is passed to
        the runner; deliveries pushed mid-turn that never reach a segment boundary
        stay pending and are re-delivered on the next pass - never silently acked,
        and never a batch range that swallows what the member did not see.
        """
        injected = self._offered.get(run.run_id)
        if not injected:
            # the turn never reached a runner segment: nothing was injected
            return
        self.store.ack_deliveries_exact(run.session_id, run.agent_id, injected)

    def _emit(self, drafts: list[EventDraft], actor_id: str = "system") -> None:
        self.control.emit(drafts, actor_id=actor_id)

    # ------------------------------------------------------------------ idle

    async def settle(self, timeout: float = 10.0) -> bool:
        """Test/CLI helper: wait until no runs are executing or queued."""
        deadline = asyncio.get_event_loop().time() + timeout
        while True:
            self._wake.set()
            queued = self.store.runs_for_session(
                self.session_id, [TurnStatus.QUEUED, TurnStatus.RUNNING])
            inflight = [t for t in self._inflight.values() if not t.done()]
            if not queued and not inflight:
                return True
            if asyncio.get_event_loop().time() > deadline:
                return False
            await asyncio.sleep(0.02)


def default_barriers() -> dict[str, asyncio.Event]:
    return {}


def fake_session(tmpdir: str | Path, spec, runners: dict[str, FakeMember],
                 catalog: UserConfig | None = None,
                 pre_authorized: set[str] | None = None,
                 require_approval: set[str] | None = None,
                 tool_executor: Callable[[str, dict[str, Any]], Any] | None = None,
                 session_id: str = "s1") -> SessionRuntime:
    """Build a ready-to-run session with scripted members (tests, examples)."""
    from .models import PermissionMode

    path = Path(tmpdir) / f"{session_id}.db"
    store = Store(path)
    if store.get_session(session_id) is None:
        store.create_session(session_id, str(tmpdir), "approved_scope")
        store.save_team_spec(session_id, spec)
        for agent in spec.agents:
            store.ensure_agent(session_id, agent.id)
    policy = PermissionPolicy(mode=PermissionMode.APPROVED_SCOPE,
                              pre_authorized=pre_authorized,
                              require_approval=require_approval)
    runtime = SessionRuntime(store, session_id, catalog or UserConfig(),
                             runners=dict(runners),
                             approvals=ApprovalGate(store, session_id, policy),
                             tool_executor=tool_executor)
    return runtime


_ = (TEAM_TOOLS, FakeMember)
