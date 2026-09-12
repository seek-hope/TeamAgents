"""Codex execution member adapter: `codex app-server` over stdio (plan section 10.2).

The adapter drives a real app-server process, maps structured events to team
progress/results/approvals, waits for confirmed interrupts, and treats an
unverifiable turn as OUTCOME_UNKNOWN rather than "probably fine".
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import os
from pathlib import Path
from typing import Any, Callable

from .agents import TurnOutcome, WakeInfo
from .models import (
    AgentSpec,
    AgentView,
    ApprovalRequest,
    ApprovalStatus,
    TurnRun,
    TurnStatus,
    new_id,
)
from .permissions import ApprovalGate

log = logging.getLogger("teamagents.codex")


class CodexError(RuntimeError):
    pass


class CodexAppServer:
    """Minimal JSON-RPC client for one app-server process."""

    def __init__(self, *, codex_bin: str = "codex", cwd: Path,
                 config_overrides: dict[str, Any] | None = None,
                 env: dict[str, str] | None = None, codex_home: str | None = None):
        self.codex_bin = codex_bin
        self.cwd = Path(cwd)
        self.config_overrides = config_overrides or {}
        self.env = env
        self.codex_home = codex_home
        self.proc: asyncio.subprocess.Process | None = None
        self._next_id = 1
        self._pending: dict[int, asyncio.Future] = {}
        self._reader_task: asyncio.Task | None = None
        self.notify: Callable[[dict], None] | None = None
        self.on_request: Callable[[dict], Any] | None = None
        self.stderr_lines: list[str] = []

    async def start(self) -> None:
        args = [self.codex_bin, "app-server"]
        for key, value in self.config_overrides.items():
            args += ["-c", f"{key}={json.dumps(value) if not isinstance(value, str) else value}"]
        env = dict(os.environ)
        if self.codex_home:
            env["CODEX_HOME"] = self.codex_home
        env.update(self.env or {})
        self.proc = await asyncio.create_subprocess_exec(
            *args, stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE, cwd=str(self.cwd), env=env,
            start_new_session=True)
        self._reader_task = asyncio.create_task(self._read_loop(), name="codex-reader")
        asyncio.create_task(self._drain_stderr(), name="codex-stderr")
        result = await self.call("initialize", {
            "clientInfo": {"name": "teamagents", "title": "TeamAgents", "version": "0.1.0"}})
        log.debug("codex app-server ready: %s", str(result)[:120])

    async def _read_loop(self) -> None:
        assert self.proc and self.proc.stdout
        while True:
            line = await self.proc.stdout.readline()
            if not line:
                break
            try:
                message = json.loads(line.decode("utf-8", errors="replace"))
            except Exception:
                continue
            if "id" in message and ("result" in message or "error" in message):
                future = self._pending.pop(int(message["id"]), None)
                if future and not future.done():
                    if "error" in message:
                        future.set_exception(CodexError(str(message["error"])))
                    else:
                        future.set_result(message.get("result"))
            elif "id" in message and "method" in message:
                await self._handle_server_request(message)
            elif "method" in message and self.notify is not None:
                with contextlib.suppress(Exception):
                    self.notify(message)
        for future in self._pending.values():
            if not future.done():
                future.set_exception(CodexError("app-server closed the connection"))
        self._pending.clear()

    async def _drain_stderr(self) -> None:
        assert self.proc and self.proc.stderr
        while True:
            line = await self.proc.stderr.readline()
            if not line:
                break
            text = line.decode("utf-8", errors="replace").strip()
            if text:
                self.stderr_lines = (self.stderr_lines + [text])[-50:]

    async def _handle_server_request(self, message: dict) -> None:
        if self.on_request is None:
            await self.respond(message["id"], {"decision": "decline"})
            return
        try:
            result = self.on_request(message)
            if asyncio.iscoroutine(result):
                result = await result
            await self.respond(message["id"], result or {})
        except Exception as e:
            log.warning("failed to answer %s: %s", message.get("method"), e)
            await self.respond(message["id"], {"decision": "decline"})

    async def call(self, method: str, params: dict | None = None,
                   timeout: float = 60) -> Any:
        if self.proc is None or self.proc.stdin is None:
            raise CodexError("app-server is not running")
        request_id = self._next_id
        self._next_id += 1
        future: asyncio.Future = asyncio.get_event_loop().create_future()
        self._pending[request_id] = future
        payload = json.dumps({"id": request_id, "method": method,
                              "params": params or {}}) + "\n"
        self.proc.stdin.write(payload.encode())
        await self.proc.stdin.drain()
        return await asyncio.wait_for(future, timeout=timeout)

    async def respond(self, request_id: Any, result: dict) -> None:
        if self.proc is None or self.proc.stdin is None:
            return
        self.proc.stdin.write(
            (json.dumps({"id": request_id, "result": result}) + "\n").encode())
        await self.proc.stdin.drain()

    async def close(self) -> None:
        if self._reader_task:
            self._reader_task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await self._reader_task
        if self.proc is not None and self.proc.returncode is None:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(os.getpgid(self.proc.pid), 15)
            try:
                await asyncio.wait_for(self.proc.wait(), timeout=5)
            except asyncio.TimeoutError:
                with contextlib.suppress(ProcessLookupError):
                    os.killpg(os.getpgid(self.proc.pid), 9)
        self.proc = None


#: terminal statuses a Codex turn can report
_TURN_STATUS_MAP = {
    "completed": TurnStatus.COMPLETED,
    "interrupted": TurnStatus.CANCELLED,
    "failed": TurnStatus.FAILED,
    "inProgress": TurnStatus.RUNNING,
}


class CodexRunner:
    """One Codex execution member: its own app-server connection and thread."""

    def __init__(self, *, agent: AgentSpec, session_id: str, workdir: Path,
                 approvals: ApprovalGate, store, sandbox: str = "workspace-write",
                 approval_policy: str = "on-request", effort: str = "xhigh",
                 model: str | None = None, codex_bin: str = "codex",
                 codex_home: str | None = None, env: dict[str, str] | None = None,
                 config_overrides: dict[str, Any] | None = None,
                 status_hook: Callable[[str, TurnStatus], None] | None = None,
                 progress_hook: Callable[[str, str], None] | None = None,
                 server_factory: Callable[..., CodexAppServer] = CodexAppServer):
        self.agent = agent
        self.session_id = session_id
        self.workdir = Path(workdir)
        self.approvals = approvals
        self.store = store
        self.sandbox = sandbox
        self.approval_policy = approval_policy
        self.effort = effort
        self.effort_fallback_used = False
        self.model = model
        self.codex_bin = codex_bin
        self.codex_home = codex_home
        self.env = env or {}
        self.config_overrides = dict(config_overrides or {})
        self.status_hook = status_hook
        self.progress_hook = progress_hook
        self.stream_hook: Callable[[str, str, str], None] | None = None
        self.server_factory = server_factory
        self.server: CodexAppServer | None = None
        self.thread_id: str | None = None
        self._states: dict[str, TurnStatus] = {}
        self._current_turn: dict[str, str] = {}
        self._turn_done: dict[str, asyncio.Future] = {}
        self._progress: dict[str, list[str]] = {}
        self._reported: dict[str, int] = {}
        self._approval_waits: dict[str, asyncio.Future] = {}
        self._approval_ids: dict[str, str] = {}
        self._queued_input: dict[str, list[str]] = {}
        self._buffered_notes: dict[str, list[dict]] = {}

    # ------------------------------------------------------------ connection

    async def _ensure_server(self) -> CodexAppServer:
        if self.server is None:
            overrides: dict[str, Any] = {}
            overrides.update(self.config_overrides)
            if self.model:
                overrides["model"] = self.model
            server = self.server_factory(cwd=self.workdir, codex_home=self.codex_home,
                                         config_overrides=overrides, env=self.env,
                                         codex_bin=self.codex_bin)
            await server.start()
            self.server = server
            self.thread_id = self.store.get_codex_thread(self.session_id, self.agent.id)
        return self.server

    async def _ensure_thread(self, server: CodexAppServer) -> str:
        if self.thread_id:
            return self.thread_id
        params: dict[str, Any] = {
            "cwd": str(self.workdir), "sandbox": self.sandbox,
            "approvalPolicy": self.approval_policy,
            "approvalsReviewer": "user", "threadSource": "appServer",
        }
        if self.model:
            params["model"] = self.model
        result = await server.call("thread/start", params)
        self.thread_id = result["thread"]["id"]
        # persist the thread before submitting a turn (plan section 10.2)
        self.store.set_codex_thread(self.session_id, self.agent.id, self.thread_id)
        return self.thread_id

    # ------------------------------------------------------- AgentRunner API

    async def start_or_resume(self, run: TurnRun, view: AgentView, gateway,
                              wake: WakeInfo | None) -> TurnOutcome:
        server = await self._ensure_server()
        thread_id = await self._ensure_thread(server)
        run_id = run.run_id
        self._states[run_id] = TurnStatus.RUNNING
        done: asyncio.Future = asyncio.get_event_loop().create_future()
        self._turn_done[run_id] = done
        self._progress[run_id] = []
        text = self._render_input(view, wake)
        params: dict[str, Any] = {
            "threadId": thread_id,
            "input": [{"type": "text", "text": text}],
            "approvalPolicy": self.approval_policy,
            "approvalsReviewer": "user",
        }
        if self.effort:
            params["effort"] = self.effort
        # handlers must be live before the turn starts: events and approval
        # requests can arrive immediately after turn/start
        server.notify = lambda message: self._on_notification(run_id, message)
        server.on_request = lambda message: self._on_request(run_id, message)
        try:
            result = await server.call("turn/start", params, timeout=60)
        except CodexError as e:
            if self.effort and not self.effort_fallback_used and "effort" in str(e).lower():
                # the selected model advertises a different effort scale: fall back to max
                self.effort_fallback_used = True
                self.effort = "max"
                params["effort"] = self.effort
                result = await server.call("turn/start", params, timeout=60)
            else:
                self._states[run_id] = TurnStatus.FAILED
                return TurnOutcome(status=TurnStatus.FAILED, error=f"CodexError: {e}")
        turn_id = result["turn"]["id"]
        self._current_turn[run_id] = turn_id
        for message in self._buffered_notes.pop(run_id, []):
            self._apply_notification(run_id, turn_id, message)
        if self._states.get(run_id) is TurnStatus.WAITING_APPROVAL:
            # an approval request beat the turn/start response: stay parked
            self.store.set_run_external_turn(run_id, turn_id)
        else:
            self.store.set_run_status(run_id, TurnStatus.RUNNING, external_turn_id=turn_id)

        try:
            await done
        except asyncio.CancelledError:
            self._states[run_id] = TurnStatus.OUTCOME_UNKNOWN
            raise
        status = self._states.get(run_id, TurnStatus.OUTCOME_UNKNOWN)
        if status is TurnStatus.WAITING_APPROVAL:
            # the turn is still live and parked on a decision
            return TurnOutcome(status=TurnStatus.WAITING_APPROVAL,
                               note=self._approval_ids.get(run_id))
        if status is TurnStatus.COMPLETED and run.task_id:
            # the adapter submits the same completion request a built-in member
            # would (plan section 6.2 / 10.2)
            summary = " ".join(self._progress.get(run_id, []))[-2000:]
            receipt = gateway.call("complete_task",
                                   {"task_id": run.task_id, "summary": summary,
                                    "result_refs": []},
                                   tool_call_id=f"{turn_id}:complete")
            if not receipt.ok:
                log.warning("completion request rejected for %s: %s",
                            run.task_id, receipt.error)
        reply = " ".join(self._progress.get(run_id, []))[-4000:] or None
        return TurnOutcome(status=status, reply_text=reply,
                           error=self._progress.get(run_id, [""])[-1]
                           if status is TurnStatus.FAILED else None)

    def _render_input(self, view: AgentView, wake: WakeInfo | None) -> str:
        from .runners import render_view
        text = render_view(view, wake, str(self.workdir))
        queued = self._queued_input.pop(view.agent_id, [])
        if queued:
            text += "\n<queued_updates>" + "\n".join(queued) + "</queued_updates>"
        text += ("\n<codex_member>\nYou are an execution member. Work the assigned "
                 "task, report progress through your own outputs; the Leader "
                 "coordinates the team. Do not attempt team-management actions.\n"
                 "</codex_member>")
        return text

    def _on_notification(self, run_id: str, message: dict) -> None:
        expected = self._current_turn.get(run_id)
        if expected is None:
            # the turn id is only known once turn/start returns: buffer instead
            # of dropping events that arrive first (fast fake/real servers do)
            self._buffered_notes.setdefault(run_id, []).append(message)
            return
        self._apply_notification(run_id, expected, message)

    def _apply_notification(self, run_id: str, expected_turn: str, message: dict) -> None:
        method = message.get("method", "")
        params = message.get("params") or {}
        turn = params.get("turn") or {}
        turn_id = turn.get("id") or params.get("turnId")
        if turn_id and turn_id != expected_turn:
            return
        if method == "item/agentMessage/delta":
            delta = params.get("delta") or ""
            self._progress.setdefault(run_id, []).append(delta)
            if delta and self.stream_hook is not None:
                self.stream_hook(run_id, self.agent.id, delta)
        elif method == "item/completed":
            item = params.get("item") or {}
            if item.get("type") == "agentMessage" and item.get("text"):
                self._progress.setdefault(run_id, []).append(item["text"])
                if self.progress_hook:
                    self.progress_hook(run_id, item["text"])
        elif method == "turn/completed":
            status = _TURN_STATUS_MAP.get(turn.get("status", ""), TurnStatus.FAILED)
            pieces = self._progress.setdefault(run_id, [])
            reported = self._reported.get(run_id, 0)
            new_text = " ".join(pieces[reported:]).strip()
            if new_text and self.progress_hook:
                self._reported[run_id] = len(pieces)
                self.progress_hook(run_id, new_text)
            self._states[run_id] = status
            self._finish(run_id)
        elif method == "error":
            self._progress.setdefault(run_id, []).append(str(params.get("message", "")))

    def _finish(self, run_id: str) -> None:
        future = self._turn_done.get(run_id)
        if future and not future.done():
            future.set_result(True)

    async def _on_request(self, run_id: str, message: dict) -> dict:
        method = message.get("method", "")
        params = message.get("params") or {}
        if not method.endswith("requestApproval"):
            if method == "item/tool/requestUserInput":
                return {"answers": []}
            return {}
        scope = {"kind": method.split("/")[1] if "/" in method else method,
                 "request": params}
        request = self.approvals.register_external(
            agent_id=self.agent.id, run_id=run_id,
            tool_call_id=str(params.get("itemId") or params.get("approvalId") or new_id("cx")),
            scope=scope)
        self._approval_ids[run_id] = request.approval_id
        wait: asyncio.Future = asyncio.get_event_loop().create_future()
        self._approval_waits[request.approval_id] = wait
        self._states[run_id] = TurnStatus.WAITING_APPROVAL
        if self.status_hook:
            self.status_hook(run_id, TurnStatus.WAITING_APPROVAL)
        decision = await wait
        self._states[run_id] = TurnStatus.RUNNING
        if self.status_hook:
            self.status_hook(run_id, TurnStatus.RUNNING)
        return {"decision": decision}

    def resolve_approval(self, approval_id: str, decision: str) -> bool:
        """Called by the runtime when the user decides (once/session/deny)."""
        wait = self._approval_waits.pop(approval_id, None)
        if wait is None or wait.done():
            return False
        mapped = {"once": "accept", "session": "acceptForSession",
                  "deny": "decline"}.get(decision, "decline")
        wait.set_result(mapped)
        return True

    async def request_interrupt(self, run_id: str) -> TurnStatus:
        turn_id = self._current_turn.get(run_id)
        server = self.server
        if server is None or turn_id is None or self.thread_id is None:
            self._states[run_id] = TurnStatus.CANCELLED
            return TurnStatus.CANCELLED
        try:
            await server.call("turn/interrupt",
                              {"threadId": self.thread_id, "turnId": turn_id}, timeout=30)
        except CodexError as e:
            log.warning("interrupt failed: %s", e)
        # cancellation is confirmed only when the turn reports a terminal state
        try:
            await asyncio.wait_for(asyncio.shield(self._turn_done[run_id]), timeout=30)
        except asyncio.TimeoutError:
            self._states[run_id] = TurnStatus.OUTCOME_UNKNOWN
        status = self._states.get(run_id, TurnStatus.OUTCOME_UNKNOWN)
        return status if status in (TurnStatus.CANCELLED, TurnStatus.COMPLETED,
                                    TurnStatus.FAILED) else TurnStatus.OUTCOME_UNKNOWN

    def query_state(self, run_id: str) -> TurnStatus | None:
        return self._states.get(run_id)

    def deliver_mid_turn(self, run_id: str, items: list[dict[str, Any]]) -> None:
        """Codex turns are not mid-turn steerable here; updates ride the next turn."""
        self._queued_input.setdefault(self.agent.id, []).extend(
            json.dumps(i.get("payload", i), ensure_ascii=False) for i in items)

    async def reconcile(self, run: TurnRun) -> TurnStatus | None:
        """After a restart: the app-server child died with us, so a turn that was
        live is unverifiable. Check the history before declaring that."""
        thread_id = self.store.get_codex_thread(self.session_id, self.agent.id)
        if thread_id is None:
            return None
        try:
            server = await self._ensure_server()
            result = await server.call("thread/read",
                                       {"threadId": thread_id, "includeTurns": True},
                                       timeout=30)
        except Exception:
            return TurnStatus.OUTCOME_UNKNOWN
        turns = ((result or {}).get("thread") or {}).get("turns") or []
        if not turns:
            return None
        last = turns[-1]
        status = _TURN_STATUS_MAP.get(last.get("status", ""), TurnStatus.OUTCOME_UNKNOWN)
        return status if status is not TurnStatus.RUNNING else TurnStatus.OUTCOME_UNKNOWN

    async def aclose(self) -> None:
        if self.server is not None:
            await self.server.close()
            self.server = None
