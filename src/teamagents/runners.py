"""DeepAgentsRunner: real Deep Agents member execution (plan section 10.1).

The runner owns one member's graph, its private checkpoint thread, its tool
surface (file tools from the Deep Agents backend, the isolated shell, MCP/web
tools and the team tools) and the middleware that injects authorized input,
enforces approvals and bounds the turn.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Annotated, Any

from langchain_core.messages import HumanMessage, ToolMessage
from langchain_core.messages import AIMessageChunk
from langchain_core.tools import BaseTool, InjectedToolCallId, tool
from langchain.agents.middleware import AgentMiddleware
from langchain.agents.middleware.types import AgentState
from langgraph.types import Command, interrupt

from deepagents import create_deep_agent
from deepagents.backends import CompositeBackend
from deepagents.middleware.subagents import GENERAL_PURPOSE_SUBAGENT

from .agents import TEAM_TOOLS, ToolGateway, TurnOutcome, WakeInfo
from .execution import (
    GuardedFilesystemBackend,
    IsolatedShellBackend,
    ReadOnlyFilesystemBackend,
    run_isolated,
)
from .models import AgentSpec, AgentView, TurnRun, TurnStatus, UserConfig
from .permissions import ApprovalGate, operation_hash
from .providers import build_chat_model, resolve_profile


class TurnLimitExceeded(RuntimeError):
    """Raised inside the graph when the per-turn model-step budget is spent."""


class TurnInterrupted(Exception):
    """Stop at a model/tool boundary after outstanding work returns."""


class TeamMemberState(AgentState):
    """Checkpointed member state. `applied_batch` brackets the last delivery
    batch that reached the conversation, so a restart can tell whether a batch
    is already inside the thread."""

    applied_batch: int


class Inbox:
    """Per-run inbox: items the member may see plus the batch applied so far."""

    def __init__(self) -> None:
        self.items: list[dict[str, Any]] = []
        self.batch = 0

    def push(self, items: list[dict[str, Any]], batch: int) -> None:
        self.items.extend(items)
        self.batch = max(self.batch, batch)

    def drain(self) -> tuple[list[dict[str, Any]], int]:
        items, batch = self.items, self.batch
        self.items = []
        return items, batch


def render_view(view: AgentView, wake: WakeInfo | None,
                workdir: str | None = None) -> str:
    """Compact text view for the member's next model call."""
    parts: list[str] = []
    if wake is not None and wake.reason != "new_input":
        parts.append("<wake reason=\"{}\">{}</wake>".format(
            wake.reason, json.dumps(wake.payload, ensure_ascii=False)))
    if view.assignment:
        parts.append("<your_tasks>" + json.dumps(
            [{"task_id": t.task_id, "description": t.description,
              "acceptance": t.acceptance, "status": t.status,
              "requester": t.requester} for t in view.assignment],
            ensure_ascii=False) + "</your_tasks>")
    for item in view.inbox_delta:
        parts.append("<inbox from=\"{}\" kind=\"{}\">{}</inbox>".format(
            item["from"], item["kind"], json.dumps(item["payload"], ensure_ascii=False)))
    if view.permitted_shared_delta:
        parts.append("<shared_space_updates>" + json.dumps(
            [{"space": e.space_id, "author": e.author, "kind": e.kind,
              "content": e.content[:500], "ref": e.ref}
             for e in view.permitted_shared_delta], ensure_ascii=False)
            + "</shared_space_updates>")
    topo = view.relevant_topology
    parts.append("<team revision=\"{}\">{}</team>".format(
        topo.get("revision"),
        json.dumps({"members": topo.get("members"),
                    "you_can_message": topo.get("can_send_to"),
                    "you_can_delegate_to": topo.get("can_delegate_to"),
                    "shared_spaces": topo.get("shared_spaces")}, ensure_ascii=False)))
    if workdir:
        parts.append(f"<your_workspace>{workdir}</your_workspace>")
    return "\n".join(parts)


#: operation shapes shared by propose_team_change / apply_topology_patch
TOPOLOGY_OPS_DOC = (
    "operations is a list of mappings, each exactly one of: "
    '{"op":"add_agent","agent":{"id","name","role","runtime_kind":"deepagents",'
    '"instructions","model_profile","tool_bindings":["files","shell",...],'
    '"skills":[],"workspace_policy":"shared"},"channels":[{"source","targets":[],'
    '"mode":"message|task|broadcast"}],"shared_spaces":[{"id","readers":[],"writers":[]}]}; '
    '{"op":"remove_agent","agent_id"}; '
    '{"op":"update_agent","agent_id","changes":{role?/instructions?/model_profile?/tool_bindings?}}; '
    '{"op":"add_channel","channel":{"source","targets":[],"mode"}}; '
    '{"op":"remove_channel","source","targets":[]}; '
    '{"op":"set_observer","observer":{"agent_id","subjects":[],"event_types":[],'
    '"payload_scope":"status|public_message|result","wake_policy":"none|on_event",'
    '"capabilities":[]},"remove":false}; '
    '{"op":"set_space_acl","space_id","readers":[],"writers":[]}. '
    "Model profiles and non-builtin tool bindings must already exist in user config."
)


TEAM_TOOL_DOCS: dict[str, str] = {
    "send_message": "Send a message to a teammate you are allowed to reach. "
                    "target='*' broadcasts where a broadcast channel exists.",
    "assign_task": "Assign a task to a teammate; returns a task_id immediately "
                   "and never waits for completion. Include acceptance criteria.",
    "complete_task": "Report the current task finished with result refs and a short "
                     "summary; the task becomes SUCCEEDED when your turn ends cleanly.",
    "wait_for_tasks": "Park this turn until the given tasks finish (or the user "
                      "sends new input). Releases your execution slot.",
    "publish_shared": "Append a structured entry (finding/decision/artifact ref) to "
                      "a shared space you can write to.",
    "read_shared": "Read shared-space entries after a sequence cursor.",
    "list_shared": "List shared spaces you can read and their entry counts.",
    "request_help": "Ask the Leader for help with your current task.",
    "propose_team_change": "Propose a team/topology change to the Leader; only the "
                           "Leader can apply it. " + TOPOLOGY_OPS_DOC,
    "apply_topology_patch": "Leader only: apply (or reject) a topology patch from a "
                            "base revision. " + TOPOLOGY_OPS_DOC,
    "cancel_task": "Leader only: cancel an unfinished or blocked task; running work stops first.",
    "cancel_run": "Leader only: request a turn to stop; side effects are not rolled back.",
    "signal_done": "Leader only: declare the current user goal complete; the runtime "
                   "verifies no work, approvals or unknown outcomes are outstanding.",
}


def build_team_tools(runner: "DeepAgentsRunner") -> list[BaseTool]:
    """Team actions. Identity comes from the runner's *current* turn
    (one active turn per member), so a cached graph never reuses a stale run."""

    async def call(name: str, args: dict[str, Any], tool_call_id: str) -> str:
        # tools stay on the event-loop thread: the runtime's single writer and
        # the member's identity resolution both assume it
        receipt = runner.current_gateway().call(name, args, tool_call_id=tool_call_id)
        return json.dumps({"ok": receipt.ok, "error": receipt.error,
                           "result": receipt.result}, ensure_ascii=False)

    @tool
    async def send_message(target: str, text: str,
                     tool_call_id: Annotated[str, InjectedToolCallId] = "") -> str:
        """Send a message to a teammate you are allowed to reach."""
        return await call("send_message", {"target": target, "text": text}, tool_call_id)

    @tool
    async def assign_task(assignee: str, description: str, acceptance: str = "",
                    dependencies: list[str] | None = None,
                    tool_call_id: Annotated[str, InjectedToolCallId] = "") -> str:
        """Assign a task to a teammate; returns a task_id immediately."""
        return await call("assign_task", {"assignee": assignee, "description": description,
                                    "acceptance": acceptance,
                                    "dependencies": dependencies or []}, tool_call_id)

    @tool
    async def complete_task(task_id: str, summary: str = "",
                      result_refs: list[str] | None = None,
                      tool_call_id: Annotated[str, InjectedToolCallId] = "") -> str:
        """Report your current task finished (applies when this turn ends cleanly)."""
        return await call("complete_task", {"task_id": task_id, "summary": summary,
                                      "result_refs": result_refs or []}, tool_call_id)

    @tool
    async def wait_for_tasks(task_ids: list[str],
                       tool_call_id: Annotated[str, InjectedToolCallId] = "") -> str:
        """Park this turn until the given tasks finish or the user sends new input."""
        return await call("wait_for_tasks", {"task_ids": task_ids}, tool_call_id)

    @tool
    async def publish_shared(space_id: str, content: str = "", kind: str = "note",
                       ref: str | None = None, supersedes: str | None = None,
                       tool_call_id: Annotated[str, InjectedToolCallId] = "") -> str:
        """Append a structured entry to a shared space you can write to."""
        return await call("publish_shared", {"space_id": space_id, "content": content,
                                       "kind": kind, "ref": ref,
                                       "supersedes": supersedes}, tool_call_id)

    @tool
    async def read_shared(space_id: str | None = None, after_sequence: int | None = None,
                    limit: int = 50,
                    tool_call_id: Annotated[str, InjectedToolCallId] = "") -> str:
        """Read shared-space entries after a sequence cursor."""
        return await call("read_shared", {"space_id": space_id,
                                    "after_sequence": after_sequence,
                                    "limit": limit}, tool_call_id)

    @tool
    async def list_shared(tool_call_id: Annotated[str, InjectedToolCallId] = "") -> str:
        """List shared spaces you can read and their entry counts."""
        return await call("list_shared", {}, tool_call_id)

    @tool
    async def request_help(message: str, task_id: str | None = None,
                     tool_call_id: Annotated[str, InjectedToolCallId] = "") -> str:
        """Ask the Leader for help with your current task."""
        return await call("request_help", {"message": message, "task_id": task_id}, tool_call_id)

    @tool
    async def propose_team_change(operations: list[dict[str, Any]], rationale: str = "",
                            tool_call_id: Annotated[str, InjectedToolCallId] = "") -> str:
        """Propose a team/topology change to the Leader (the Leader decides)."""
        return await call("propose_team_change", {"operations": operations,
                                            "rationale": rationale}, tool_call_id)

    @tool
    async def apply_topology_patch(operations: list[dict[str, Any]] | None = None,
                             base_revision: int | None = None,
                             patch_id: str | None = None, reject: bool = False,
                             tool_call_id: Annotated[str, InjectedToolCallId] = "") -> str:
        """Leader only: apply (or reject) a topology patch."""
        return await call("apply_topology_patch", {"operations": operations,
                                             "base_revision": base_revision,
                                             "patch_id": patch_id, "reject": reject},
                    tool_call_id)

    @tool
    async def signal_done(summary: str = "",
                    tool_call_id: Annotated[str, InjectedToolCallId] = "") -> str:
        """Leader only: declare the current user goal complete."""
        return await call("signal_done", {"summary": summary}, tool_call_id)

    @tool
    async def cancel_task(task_id: str,
                          tool_call_id: Annotated[str, InjectedToolCallId] = "") -> str:
        """Leader only: cancel an unfinished or blocked task."""
        return await call("cancel_task", {"task_id": task_id}, tool_call_id)

    @tool
    async def cancel_run(run_id: str,
                         tool_call_id: Annotated[str, InjectedToolCallId] = "") -> str:
        """Leader only: request a turn to stop."""
        return await call("cancel_run", {"run_id": run_id}, tool_call_id)

    tools = [send_message, assign_task, complete_task, wait_for_tasks, publish_shared,
             read_shared, list_shared, request_help, propose_team_change,
             apply_topology_patch, signal_done, cancel_task, cancel_run]
    for t in tools:
        t.description = TEAM_TOOL_DOCS.get(t.name, t.description)
    return tools


class TeamAgentMiddleware(AgentMiddleware):
    """Injects authorized input, bounds the turn, enforces approvals."""

    name = "teamagents"
    state_schema = TeamMemberState

    def __init__(self, *, runner: "DeepAgentsRunner", approvals: ApprovalGate,
                 max_model_steps: int):
        self.runner = runner
        self.approvals = approvals
        self.max_model_steps = max_model_steps
        self.model_steps = 0

    def reset(self) -> None:
        self.model_steps = 0

    async def abefore_model(self, state, runtime):
        if self.model_steps >= self.max_model_steps:
            return None
        items, batch = self.runner.current_inbox().drain()
        if not items:
            return None
        text = "\n".join(
            "<update from=\"{}\" kind=\"{}\">{}</update>".format(
                i.get("from"), i.get("kind"),
                json.dumps(i.get("payload", {}), ensure_ascii=False))
            for i in items)
        return {"messages": [HumanMessage(content=text)], "applied_batch": batch}

    async def awrap_model_call(self, request, handler):
        self.runner.raise_if_interrupted()
        self.model_steps += 1
        if self.model_steps > self.max_model_steps:
            raise TurnLimitExceeded(
                f"model-step limit {self.max_model_steps} reached for this turn")
        return await handler(request)

    async def awrap_tool_call(self, request, handler):
        self.runner.raise_if_interrupted()
        tool_call = request.tool_call
        name = tool_call["name"]
        if name in TEAM_TOOLS:
            result = await handler(request)
            if name == "wait_for_tasks":
                # durable pause: the graph stops here and resumes on the same
                # run_id when wait conditions resolve (plan section 6.2)
                try:
                    payload = json.loads(result.content)
                except Exception:
                    payload = {}
                waiting = (payload.get("result") or {}).get("waiting")
                if waiting:
                    interrupt({"waiting_on": (payload.get("result") or {}).get("task_ids", [])})
            return result
        args = tool_call.get("args") or {}
        run_id = self.runner.current_run_id
        tool_call_id = tool_call.get("id") or f"{run_id}:{name}"
        if name in self.runner.bound_tool_names():
            # MCP/web tools exist only because the user configured and bound
            # them to this member: the binding is the authorization (section 12.1)
            return await handler(request)
        decision, approval = self.approvals.check(
            self.runner.agent.id, run_id, name, args, tool_call_id)
        if approval is None:
            if decision.allow:
                return await handler(request)
            return ToolMessage(content=f"Blocked by team permissions: {decision.reason}",
                               tool_call_id=tool_call_id, name=name, status="error")
        status = approval.status.name
        if status == "DENIED":
            return ToolMessage(
                content="The user denied this operation. Do not retry it; choose "
                        "another approach or ask the user.",
                tool_call_id=tool_call_id, name=name, status="error")
        if status in ("APPROVED_ONCE", "APPROVED_SESSION"):
            result = await handler(request)
            if status == "APPROVED_ONCE":
                self.approvals.consume_once(approval.approval_id)
            return result
        answer = interrupt({"approval_required": approval.requested_scope,
                            "approval_id": approval.approval_id})
        allow = bool(answer.get("allow")) if isinstance(answer, dict) else bool(answer)
        if not allow:
            return ToolMessage(
                content="The user denied this operation. Do not retry it; choose "
                        "another approach or ask the user.",
                tool_call_id=tool_call_id, name=name, status="error")
        if not self.approvals.recheck(approval.approval_id, name, args):
            return ToolMessage(
                content="Approval no longer matches this call (parameters or policy "
                        "changed). Request approval again.",
                tool_call_id=tool_call_id, name=name, status="error")
        result = await handler(request)
        self.approvals.consume_once(approval.approval_id)
        return result


class SubagentGate(AgentMiddleware):
    """Permission ceiling for private subagents (plan section 6.4).

    A private subagent belongs to the member: it may use everything the
    member's policy already allows (including session-scoped approvals) and its
    model calls consume the member's own step budget. Operations that would
    need a *new* user approval are refused outright -- a nested `interrupt()`
    raised inside the `task` tool cannot reliably resume (replay loses the
    approved call, `recheck` then rejects it), and leaving a pending approval
    the user cannot act on would block `signal_done`. The member's own turn can
    still request the same operation.
    """

    name = "teamagents-subagent"

    def __init__(self, *, runner: "DeepAgentsRunner", approvals: ApprovalGate,
                 steps: TeamAgentMiddleware):
        self.runner = runner
        self.approvals = approvals
        self.steps = steps

    async def awrap_model_call(self, request, handler):
        self.runner.raise_if_interrupted()
        self.steps.model_steps += 1
        if self.steps.model_steps > self.steps.max_model_steps:
            raise TurnLimitExceeded(
                f"model-step limit {self.steps.max_model_steps} reached for this turn")
        return await handler(request)

    async def awrap_tool_call(self, request, handler):
        self.runner.raise_if_interrupted()
        name = request.tool_call["name"]
        args = request.tool_call.get("args") or {}
        tool_call_id = request.tool_call.get("id") or f"subagent:{name}"
        if name in TEAM_TOOLS:
            return ToolMessage(
                content="Team tools are not available to private subagents; the "
                        "member performs team actions in its own turn.",
                tool_call_id=tool_call_id, name=name, status="error")
        if name in self.runner.bound_tool_names():
            # MCP/web tools exist only because the user bound them (section 12.1)
            return await handler(request)
        decision = self.approvals.policy.evaluate(name, args)
        session_ok = self.approvals.store.find_session_approval(
            self.approvals.session_id, operation_hash(name, args)) is not None
        if decision.allow or session_ok:
            return await handler(request)
        return ToolMessage(
            content=f"Blocked by team permissions: {name} needs explicit user "
                    "approval and a private subagent cannot raise approval "
                    "requests. Ask the member to perform this operation in its "
                    "own turn, or choose an approach already within scope.",
            tool_call_id=tool_call_id, name=name, status="error")


class DeepAgentsRunner:
    """One built-in member: its own persistent Deep Agents graph."""

    def __init__(self, *, agent: AgentSpec, catalog: UserConfig, session_id: str,
                 workdir: Path, artifacts_dir: Path, checkpointer,
                 approvals: ApprovalGate, extra_tools: list[BaseTool] | None = None,
                 skills_dirs: list[Path] | None = None,
                 memory_files: list[Path] | None = None,
                 extra_rw: list[Path] | None = None, model_override=None):
        self.agent = agent
        self.catalog = catalog
        self.session_id = session_id
        self.workdir = Path(workdir)
        self.artifacts_dir = Path(artifacts_dir)
        self.checkpointer = checkpointer
        self.approvals = approvals
        self.extra_tools = extra_tools or []
        self.skills_dirs = [Path(p) for p in (skills_dirs or [])]
        self.memory_files = [Path(p) for p in (memory_files or [])]
        self.extra_rw = [Path(p) for p in (extra_rw or [])]
        self.model_override = model_override
        self._graph = None
        self._graph_revision: int | None = None
        self._middleware: TeamAgentMiddleware | None = None
        self._states: dict[str, TurnStatus] = {}
        self._inboxes: dict[str, Inbox] = {}
        self._threads: dict[str, str] = {}
        self._paused_kind: dict[str, str] = {}
        self._interrupt_requested: set[str] = set()
        self._effort_fallback_used = False
        self._active_run: TurnRun | None = None
        self._active_gateway: ToolGateway | None = None
        self._bound_tools: list[BaseTool] | None = None
        self.stream_hook = None

    # -- current-turn accessors (tools/middleware resolve through these) ------

    @property
    def current_run_id(self) -> str:
        assert self._active_run is not None, "no active turn for this member"
        return self._active_run.run_id

    def current_gateway(self) -> ToolGateway:
        assert self._active_gateway is not None, "no active gateway for this member"
        return self._active_gateway

    def current_inbox(self) -> Inbox:
        return self._inboxes[self.current_run_id]

    # ------------------------------------------------------------- building

    def _thread_id(self, run: TurnRun) -> str:
        epoch = (run.context_ref or "ctx:0").rsplit(":", 1)[-1]
        return f"{self.session_id}:{self.agent.id}:{epoch}"

    def _build_backend(self) -> CompositeBackend:
        # every route is guarded (real-path containment, plan section 12.2):
        # artifacts stay writable, memory/skills are user-owned and read-only
        routes: dict[str, Any] = {
            # route keys end with '/' — CompositeBackend joins them verbatim
            "/artifacts/": GuardedFilesystemBackend(root_dir=self.artifacts_dir),
        }
        for i, skills_dir in enumerate(self.skills_dirs):
            if skills_dir.is_dir():
                routes[f"/skills/{i}/"] = ReadOnlyFilesystemBackend(
                    root_dir=skills_dir, virtual_prefix=f"/skills/{i}/")
        for i, memory_file in enumerate(self.memory_files):
            if memory_file.is_file():
                routes[f"/memory/{i}/"] = ReadOnlyFilesystemBackend(
                    root_dir=memory_file.parent, virtual_prefix=f"/memory/{i}/")
        default = IsolatedShellBackend(self.workdir, artifacts_dir=self.artifacts_dir,
                                       extra_rw=self.extra_rw, network=False)
        return CompositeBackend(default=default, routes=routes)

    def _shell_tool(self) -> BaseTool:
        runner = self

        @tool
        async def shell(command: str, timeout: int = 120, network: bool = False,
                  tool_call_id: Annotated[str, InjectedToolCallId] = "") -> str:
            """Run a shell command in the isolated Linux sandbox (no network by
            default; network=true requires user approval). Output is capped and
            long output is stored under /artifacts."""
            # permission (including network escalation) is decided once, by
            # TeamAgentMiddleware.awrap_tool_call, before this body runs
            import asyncio
            result = await asyncio.to_thread(
                run_isolated, command, workdir=runner.workdir, timeout=timeout,
                network=network, extra_rw=runner.extra_rw,
                artifact_dir=runner.artifacts_dir)
            return json.dumps({"ok": True, "exit_code": result.exit_code,
                               "output": result.output,
                               "truncated": result.truncated,
                               "artifact": result.artifact}, ensure_ascii=False)

        return shell

    def _general_purpose_spec(self, model, tools: list[BaseTool]) -> dict[str, Any]:
        """Explicit `general-purpose` spec, replacing the one deepagents auto-adds.

        The auto-added default carried no TeamAgentMiddleware, so anything a
        private subagent did (including shell network escalation) skipped both
        the approval gate and the step budget. This spec keeps the capability
        but binds it to the member: `SubagentGate` applies the same policy and
        shares the step counter; the subagent gets the execution tools only
        (team-coordination tools stay with the member's own turn).
        """
        return {
            "name": GENERAL_PURPOSE_SUBAGENT["name"],
            "description": GENERAL_PURPOSE_SUBAGENT["description"],
            "system_prompt": GENERAL_PURPOSE_SUBAGENT["system_prompt"],
            "model": model,
            "tools": [t for t in tools if t.name not in TEAM_TOOLS],
            "middleware": [SubagentGate(runner=self, approvals=self.approvals,
                                        steps=self._middleware)],
        }

    def _ensure_graph(self, run: TurnRun):
        if self._graph is not None and self._graph_revision == run.config_revision:
            return self._graph
        if self.model_override is not None:
            model = self.model_override
        else:
            profile = resolve_profile(self.catalog, self.agent.model_profile)
            model = build_chat_model(profile)
        # the turn bound is spec data, never a literal: read it at graph (re)build
        limits = self.approvals.store.load_team_spec(self.session_id).limits
        self._middleware = TeamAgentMiddleware(
            runner=self, approvals=self.approvals,
            max_model_steps=limits.max_model_steps_per_turn)
        tools = [*build_team_tools(self), self._shell_tool(),
                 *(self._bound_tools or []), *self.extra_tools]
        skills = [f"/skills/{i}/" for i, d in enumerate(self.skills_dirs) if d.is_dir()]
        memory = [f"/memory/{i}/{f.name}" for i, f in enumerate(self.memory_files)
                  if f.is_file()]
        self._graph = create_deep_agent(
            model=model,
            tools=tools,
            system_prompt=self.agent.instructions or None,
            middleware=[self._middleware],
            backend=self._build_backend(),
            subagents=[self._general_purpose_spec(model, tools)],
            skills=skills or None,
            memory=memory or None,
            checkpointer=self.checkpointer,
            name=self.agent.id,
        )
        self._graph_revision = run.config_revision
        return self._graph

    # ------------------------------------------------------- AgentRunner API

    async def start_or_resume(self, run: TurnRun, view: AgentView, gateway: ToolGateway,
                              wake: WakeInfo | None) -> TurnOutcome:
        self._active_run = run
        self._active_gateway = gateway
        if self._bound_tools is None:
            from .tools import build_bound_tools
            try:
                self._bound_tools = await build_bound_tools(
                    self.catalog, list(self.agent.tool_bindings))
            except Exception:
                raise
        graph = self._ensure_graph(run)
        if self._middleware is not None:
            self._middleware.reset()
        inbox = self._inboxes.setdefault(run.run_id, Inbox())
        # the rendered input is authoritative for what this segment already saw;
        # anything pushed while the run is live is injected at the next model call
        inbox.items.clear()
        inbox.batch = view.batch_no
        thread_id = self._threads.setdefault(run.run_id, self._thread_id(run))
        cfg = {"configurable": {"thread_id": thread_id}}
        self._states[run.run_id] = TurnStatus.RUNNING
        try:
            paused = self._paused_kind.get(run.run_id)
            if paused == "approval":
                allow = not (wake.payload.get("denied", False) if wake else False)
                inbox.push(view.inbox_delta, view.batch_no)
                graph_input = Command(resume={"allow": allow,
                                              "decisions": (wake.payload.get("decisions", [])
                                                            if wake else [])})
            elif paused == "waiting":
                inbox.push(view.inbox_delta, view.batch_no)
                graph_input = Command(resume={"resumed": True,
                                              "results": (wake.payload.get("results", {})
                                                          if wake else {})})
            else:
                text = render_view(view, wake, str(self.workdir))
                graph_input = {"messages": [HumanMessage(content=text)]}
            result, reply = await self._stream_graph(graph, graph_input, cfg, run)
        except TurnInterrupted:
            self._states[run.run_id] = TurnStatus.CANCELLED
            return TurnOutcome(status=TurnStatus.CANCELLED)
        except TurnLimitExceeded as e:
            self._states[run.run_id] = TurnStatus.FAILED
            return TurnOutcome(status=TurnStatus.FAILED, error=str(e), note="turn_limit")
        except Exception as e:
            # a provider that rejects the configured reasoning effort maps to `max`
            # once; the graph is then rebuilt with the accepted value
            if (self._effort_fallback_used or not _looks_like_effort_error(e)
                    or not self._switch_effort_to_max()):
                self._states[run.run_id] = TurnStatus.FAILED
                return TurnOutcome(status=TurnStatus.FAILED,
                                   error=f"{type(e).__name__}: {e}")
            self._effort_fallback_used = True
            try:
                graph = self._ensure_graph(run)
                result, reply = await self._stream_graph(graph, graph_input, cfg, run)
            except TurnLimitExceeded as retry_error:
                self._states[run.run_id] = TurnStatus.FAILED
                return TurnOutcome(status=TurnStatus.FAILED, error=str(retry_error),
                                   note="turn_limit")
            except Exception as retry_error:
                self._states[run.run_id] = TurnStatus.FAILED
                return TurnOutcome(status=TurnStatus.FAILED,
                                   error=f"{type(retry_error).__name__}: {retry_error}")
        interrupts = result.get("__interrupt__") if isinstance(result, dict) else None
        if interrupts:
            value = getattr(interrupts[0], "value", {}) or {}
            if "approval_required" in value:
                self._paused_kind[run.run_id] = "approval"
                self._states[run.run_id] = TurnStatus.WAITING_APPROVAL
                return TurnOutcome(status=TurnStatus.WAITING_APPROVAL,
                                   note=value.get("approval_id"))
            self._paused_kind[run.run_id] = "waiting"
            self._states[run.run_id] = TurnStatus.WAITING_TASK
            return TurnOutcome(status=TurnStatus.WAITING_TASK)
        self._paused_kind.pop(run.run_id, None)
        self._states[run.run_id] = TurnStatus.COMPLETED
        return TurnOutcome(status=TurnStatus.COMPLETED, reply_text=reply)

    def _switch_effort_to_max(self) -> bool:
        """Rewrite this member's profile effort to `max` and drop the cached graph."""
        if self.model_override is not None:
            return False
        profile = self.catalog.models.get(self.agent.model_profile)
        if profile is None:
            return False
        options = dict(profile.generation_options)
        if options.get("reasoning_effort") in (None, "max", "max".upper()):
            return False
        options["reasoning_effort"] = "max"
        self.catalog.models[self.agent.model_profile] = profile.model_copy(
            update={"generation_options": options})
        self._graph = None
        self._graph_revision = None
        return True

    async def _stream_graph(self, graph, graph_input, cfg, run: TurnRun):
        """Run one graph segment, forwarding model text to the UI sink and
        returning (final state, reply text). Deltas are transient by design."""
        state: dict | None = None
        parts: list[str] = []
        async for mode, payload in graph.astream(
                graph_input, cfg, stream_mode=["messages", "values"]):
            if mode == "messages":
                chunk, _meta = payload
                if not isinstance(chunk, AIMessageChunk):
                    continue
                if getattr(chunk, "tool_call_chunks", None) or getattr(chunk, "tool_calls", None):
                    continue
                text = chunk.content if isinstance(chunk.content, str) else ""
                if text:
                    parts.append(text)
                    if self.stream_hook is not None:
                        self.stream_hook(run.run_id, self.agent.id, text)
            elif mode == "values":
                state = payload
        reply = "".join(parts).strip()
        return state or {}, reply or None

    def raise_if_interrupted(self) -> None:
        run = self._active_run
        if run is not None and (run.run_id in self._interrupt_requested
                or self.approvals.store.run_cancel_requested(run.run_id)):
            raise TurnInterrupted()

    async def request_interrupt(self, run_id: str) -> TurnStatus:
        # Do not claim a stop while a tool (including a threaded shell) is still
        # executing. Its completion is the next safe cancellation boundary.
        import asyncio
        self._interrupt_requested.add(run_id)
        while self._states.get(run_id) is TurnStatus.RUNNING:
            await asyncio.sleep(0.01)
        return TurnStatus.CANCELLED

    def query_state(self, run_id: str) -> TurnStatus | None:
        return self._states.get(run_id)

    def deliver_mid_turn(self, run_id: str, items: list[dict[str, Any]]) -> None:
        inbox = self._inboxes.setdefault(run_id, Inbox())
        inbox.push(items, inbox.batch)

    def bound_tool_names(self) -> set[str]:
        """Tools the user bound to this member (their binding is the authorization)."""
        return {t.name for t in [*(self._bound_tools or []), *self.extra_tools]}

    async def reconcile(self, run: TurnRun) -> TurnStatus | None:
        """After a restart: believe this member's own checkpoint, not the DB row."""
        if self._graph is None:
            return None
        thread_id = self._threads.get(run.run_id) or self._thread_id(run)
        cfg = {"configurable": {"thread_id": thread_id}}
        try:
            state = await self._graph.aget_state(cfg)
        except Exception:
            return None
        if state is None or not getattr(state, "values", None):
            return None
        applied = int(state.values.get("applied_batch") or 0)
        if applied:
            self.approvals.store.ack_deliveries(self.session_id, self.agent.id, applied)
        if state.next:
            kind = "approval"
            for task in getattr(state, "tasks", ()) or ():
                for intr in getattr(task, "interrupts", ()) or ():
                    value = getattr(intr, "value", {}) or {}
                    kind = "approval" if "approval_required" in value else "waiting"
            self._paused_kind[run.run_id] = kind
            return (TurnStatus.WAITING_APPROVAL if kind == "approval"
                    else TurnStatus.WAITING_TASK)
        return TurnStatus.COMPLETED
def _looks_like_effort_error(error: Exception) -> bool:
    text = str(error).lower()
    return any(token in text for token in
               ("reasoning_effort", "reasoning effort", "effort", "unsupported value"))
