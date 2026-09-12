"""Member execution: the small AgentRunner contract, the team tool gateway,
and the scripted fake member used for deterministic acceptance tests.

Two concrete runners exist in the first release: DeepAgentsRunner (P3) and
CodexRunner (P5). The fake runner exercises the same gateway as both.
"""

from __future__ import annotations

import asyncio
import copy
import json
from dataclasses import dataclass, field
from typing import Any, Callable, Protocol

from .control import Control
from .models import (
    ActionKind,
    AgentView,
    ApprovalStatus,
    Receipt,
    TeamAction,
    TurnRun,
    TurnStatus,
)
from .permissions import ApprovalGate

#: team tool name -> action kind
TEAM_TOOLS: dict[str, ActionKind] = {
    "send_message": ActionKind.SEND_MESSAGE,
    "assign_task": ActionKind.ASSIGN_TASK,
    "complete_task": ActionKind.COMPLETE_TASK,
    "wait_for_tasks": ActionKind.WAIT_FOR_TASKS,
    "publish_shared": ActionKind.PUBLISH_SHARED,
    "read_shared": ActionKind.READ_SHARED,
    "list_shared": ActionKind.LIST_SHARED,
    "request_help": ActionKind.REQUEST_HELP,
    "propose_team_change": ActionKind.PROPOSE_TEAM_CHANGE,
    "apply_topology_patch": ActionKind.APPLY_TOPOLOGY_PATCH,
    "signal_done": ActionKind.SIGNAL_DONE,
}


@dataclass
class WakeInfo:
    reason: str                      # task_results | user_input | approval | new_task
    payload: dict[str, Any] = field(default_factory=dict)


@dataclass
class TurnOutcome:
    status: TurnStatus
    error: str | None = None
    note: str | None = None
    reply_text: str | None = None


class AgentRunner(Protocol):
    """Start/resume a turn, subscribe progress, request interrupt, query state."""

    async def start_or_resume(self, run: TurnRun, view: AgentView, gateway: "ToolGateway",
                              wake: WakeInfo | None) -> TurnOutcome: ...

    async def request_interrupt(self, run_id: str) -> TurnStatus: ...

    def query_state(self, run_id: str) -> TurnStatus | None: ...

    def deliver_mid_turn(self, run_id: str, items: list[dict]) -> None: ...


class ToolGateway:
    """The single path for a member's tool calls, team actions and approvals.

    Identity is injected here (agent_id/run_id from the runtime), never trusted
    from model-provided fields (plan section 5.2).
    """

    def __init__(self, control: Control, session_id: str, agent_id: str,
                 run_id: str, approvals: ApprovalGate,
                 executor: Callable[[str, dict[str, Any]], Any] | None = None):
        self.control = control
        self.session_id = session_id
        self.agent_id = agent_id
        self.run_id = run_id
        self.approvals = approvals
        self.executor = executor
        self._seq = 0
        self.pending_approval_id: str | None = None

    def call(self, tool_name: str, args: dict[str, Any],
             tool_call_id: str) -> Receipt:
        """Execute one tool call. `tool_call_id` must be stable for the same
        logical call across retries/resumes: action ids derive from it, and the
        control layer replays the original receipt for repeats (section 7)."""
        self._seq += 1
        call_id = f"{self.run_id}:{tool_call_id}"
        if tool_name in TEAM_TOOLS:
            action = TeamAction(
                action_id=call_id,
                session_id=self.session_id,
                actor_id=self.agent_id,
                run_id=self.run_id,
                kind=TEAM_TOOLS[tool_name],
                payload=dict(args),
            )
            return self.control.submit(action)
        return self._execute_tool(tool_name, args, call_id)

    def _execute_tool(self, tool_name: str, args: dict[str, Any], call_id: str) -> Receipt:
        action = TeamAction(
            action_id=call_id, session_id=self.session_id, actor_id=self.agent_id,
            run_id=self.run_id, kind=ActionKind.COMPLETE_TASK, payload={},
        )
        decision, approval = self.approvals.check(
            self.agent_id, self.run_id, tool_name, args, call_id)
        if approval is not None:
            self.pending_approval_id = approval.approval_id
            return Receipt(action_id=call_id, ok=False, kind=action.kind,
                           error="approval_required",
                           result={"approval_id": approval.approval_id,
                                   "scope": approval.requested_scope})
        if not decision.allow:
            return Receipt(action_id=call_id, ok=False, kind=action.kind,
                           error=decision.reason or "operation not permitted")
        if self.executor is None:
            return Receipt(action_id=call_id, ok=False, kind=action.kind,
                           error=f"no executor for tool {tool_name!r}")
        try:
            output = self.executor(tool_name, args)
        except Exception as e:  # tool failure is a receipt, not a crash
            return Receipt(action_id=call_id, ok=False, kind=action.kind,
                           error=f"{type(e).__name__}: {e}")
        return Receipt(action_id=call_id, ok=True, kind=action.kind,
                       result={"output": output})


# ---------------------------------------------------------------------------
# Scripted fake member
# ---------------------------------------------------------------------------


def _resolve_refs(item: Any, ctx: dict[str, Any]) -> Any:
    """Templates in fake scripts: "$r0.result.task_id", "$inbox0.payload.task_id",
    "$run.task_id"."""
    if isinstance(item, str) and item.startswith("$"):
        head, _, path = item[1:].partition(".")
        value: Any = ctx.get(head)
        for part in [p for p in path.split(".") if p]:
            if isinstance(value, dict):
                value = value.get(part)
            elif isinstance(value, list) and part.isdigit():
                value = value[int(part)] if int(part) < len(value) else None
            else:
                value = None
        return value
    if isinstance(item, dict):
        return {k: _resolve_refs(v, ctx) for k, v in item.items()}
    if isinstance(item, list):
        return [_resolve_refs(v, ctx) for v in item]
    return item


class FakeMember:
    """Deterministic member driven by a script of steps.

    Steps: ("call", tool, args) | ("barrier", name) | ("sleep", seconds)
           ("wait",) | ("end",) | ("fail", msg) | ("inbox",) -> receipt of injected items
    """

    def __init__(self, agent_id: str, script: list[tuple] | None = None,
                 barriers: dict[str, "asyncio.Barrier"] | None = None):
        self.agent_id = agent_id
        self.script = list(script or [])
        self.cursor = 0
        self.barriers = barriers if barriers is not None else {}
        self.results: list[Receipt] = []
        self.observed_inbox: list[dict] = []
        self.state: dict[str, TurnStatus] = {}
        self._mid_turn: dict[str, list[dict]] = {}
        self.cancelled: set[str] = set()

    # -- AgentRunner contract -------------------------------------------------

    async def start_or_resume(self, run: TurnRun, view: AgentView, gateway: ToolGateway,
                              wake: WakeInfo | None) -> TurnOutcome:
        self.state[run.run_id] = TurnStatus.RUNNING
        self.last_view = view
        ctx: dict[str, Any] = {"run": {"task_id": run.task_id, "run_id": run.run_id,
                                       "agent_id": run.agent_id, "wake": wake.reason if wake else None}}
        for i, item in enumerate(view.inbox_delta):
            ctx[f"inbox{i}"] = item
        for i, receipt in enumerate(self.results):
            ctx[f"r{i}"] = {"ok": receipt.ok, "error": receipt.error,
                            "result": receipt.result, "kind": str(receipt.kind)}
        while self.cursor < len(self.script):
            step = self.script[self.cursor]
            match step[0]:
                case "call":
                    _, tool, args = step
                    resolved = _resolve_refs(copy.deepcopy(args), ctx)
                    receipt = gateway.call(tool, resolved, tool_call_id=f"step{self.cursor}")
                    self.results.append(receipt)
                    ctx[f"r{len(self.results) - 1}"] = {
                        "ok": receipt.ok, "error": receipt.error,
                        "result": receipt.result, "kind": str(receipt.kind)}
                    self.cursor += 1
                    if receipt.error == "approval_required":
                        self.state[run.run_id] = TurnStatus.WAITING_APPROVAL
                        return TurnOutcome(status=TurnStatus.WAITING_APPROVAL,
                                           note=str(receipt.result.get("approval_id")))
                    if not receipt.ok:
                        # tool error: member sees it and continues (models do this too)
                        continue
                case "barrier":
                    barrier = self.barriers.setdefault(step[1], asyncio.Barrier(2))
                    await self._await_or_cancel(asyncio.ensure_future(barrier.wait()),
                                                15.0, run.run_id)
                    self.cursor += 1
                case "sleep":
                    await self._await_or_cancel(asyncio.ensure_future(
                        asyncio.sleep(float(step[1]))), float(step[1]) + 1.0, run.run_id)
                    self.cursor += 1
                case "inbox":
                    items = list(view.inbox_delta) + self._mid_turn.pop(run.run_id, [])
                    self.observed_inbox.extend(items)
                    self.results.append(Receipt(action_id=f"inbox:{self.cursor}", ok=True,
                                                kind=ActionKind.SEND_MESSAGE,
                                                result={"injected": items}))
                    self.cursor += 1
                case "wait":
                    self.cursor += 1
                    self.state[run.run_id] = TurnStatus.WAITING_TASK
                    return TurnOutcome(status=TurnStatus.WAITING_TASK)
                case "end":
                    self.cursor += 1
                    self.state[run.run_id] = TurnStatus.COMPLETED
                    return TurnOutcome(status=TurnStatus.COMPLETED)
                case "fail":
                    self.cursor += 1
                    self.state[run.run_id] = TurnStatus.FAILED
                    return TurnOutcome(status=TurnStatus.FAILED, error=str(step[1]))
                case _:
                    self.cursor += 1
            if run.run_id in self.cancelled:
                self.state[run.run_id] = TurnStatus.CANCELLED
                return TurnOutcome(status=TurnStatus.CANCELLED, note="interrupted")
        self.state[run.run_id] = TurnStatus.COMPLETED
        return TurnOutcome(status=TurnStatus.COMPLETED)

    async def _await_or_cancel(self, task: "asyncio.Future", timeout: float,
                               run_id: str) -> None:
        """Cancellable wait: stop promptly when the runtime requests an interrupt."""
        try:
            deadline = asyncio.get_event_loop().time() + timeout
            while True:
                done, _ = await asyncio.wait({task}, timeout=0.02)
                if done:
                    return
                if run_id in self.cancelled:
                    return
                if asyncio.get_event_loop().time() > deadline:
                    raise asyncio.TimeoutError(f"step timed out after {timeout}s")
        finally:
            if not task.done():
                task.cancel()

    async def request_interrupt(self, run_id: str) -> TurnStatus:
        self.cancelled.add(run_id)
        self.state[run_id] = TurnStatus.CANCELLED
        return TurnStatus.CANCELLED

    def query_state(self, run_id: str) -> TurnStatus | None:
        return self.state.get(run_id)

    def deliver_mid_turn(self, run_id: str, items: list[dict]) -> None:
        self._mid_turn.setdefault(run_id, []).extend(items)

    # -- test helpers ---------------------------------------------------------

    def remaining(self) -> int:
        return max(0, len(self.script) - self.cursor)
