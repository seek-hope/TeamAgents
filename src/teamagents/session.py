"""Session bootstrap: directories, default team, runtime wiring."""

from __future__ import annotations

import contextlib
import fcntl
import os
from pathlib import Path

from .config import (
    default_session_id,
    load_user_config,
    permission_mode_from_config,
    sessions_dir,
)
from .models import (
    AgentSpec,
    PermissionMode,
    RuntimeKind,
    SharedSpaceSpec,
    TeamSpec,
    WorkspacePolicy,
)
from .permissions import ApprovalGate, PermissionPolicy
from .runtime import SessionRuntime
from .storage import Store

LEADER_INSTRUCTIONS = """\
You are the Leader of a team of agents. Understand the user's goal, decide
whether to work alone or build a team, delegate with assign_task, coordinate
with send_message, and report completion with signal_done. Keep task descriptions
specific, include acceptance criteria, and never bypass runtime permissions.
"""


def default_leader_spec(name: str = "Leader", instructions: str = LEADER_INSTRUCTIONS,
                        profile: str = "leader_main",
                        tools: list[str] | None = None) -> TeamSpec:
    """Single-Leader team is the valid starting point (section 14)."""
    return TeamSpec(
        leader_id="leader",
        agents=[AgentSpec(
            id="leader", name=name, role="leader", runtime_kind=RuntimeKind.DEEPAGENTS,
            instructions=instructions, model_profile=profile,
            tool_bindings=tools or ["files", "shell", "web"],
        )],
        shared_spaces=[SharedSpaceSpec(id="main", readers=["leader"], writers=["leader"])],
    )


def session_paths(session_id: str) -> dict[str, Path]:
    base = sessions_dir() / session_id
    return {"base": base, "db": base / "team.db", "artifacts": base / "artifacts",
            "locks": base / "locks"}


def acquire_session_lock(paths: dict[str, Path]):
    """Take the local session file lock; the fd is released when closed."""
    from .sessions import SessionInUse

    paths["base"].mkdir(parents=True, exist_ok=True)
    handle = os.open(paths["base"] / "session.lock", os.O_CREAT | os.O_RDWR, 0o600)
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError as e:
        os.close(handle)
        raise SessionInUse(
            f"session is already running in another process (lock: "
            f"{paths['base'] / 'session.lock'}); exit that process or use "
            f"--resume with a different session") from e
    return handle


def _member_workspace(paths: dict[str, Path], agent: AgentSpec, cwd: Path) -> Path:
    """Where a member works, per its workspace policy (plan section 12.3)."""
    from .workspace import prepare

    member_dir = paths["base"] / "members" / agent.id
    member_dir.mkdir(parents=True, exist_ok=True)
    workspace = prepare(agent, cwd, member_dir)
    if workspace.note:
        import logging
        logging.getLogger("teamagents.workspace").warning(
            "member %s: %s", agent.id, workspace.note)
    return workspace.path


def _skills_and_memory(catalog, cwd: Path, paths: dict[str, Path], agent: AgentSpec):
    skills_dirs = [Path(p).expanduser() for p in catalog.skills_paths]
    skills_dirs += [cwd / ".teamagents" / "skills",
                    paths["base"] / "members" / agent.id / "skills"]
    memory_files = [Path(p).expanduser() for p in catalog.instruction_files]
    for candidate in (cwd / "AGENTS.md", Path.home() / ".config" / "teamagents" / "AGENTS.md"):
        if candidate.is_file():
            memory_files.append(candidate)
    return skills_dirs, memory_files


async def open_session(cwd: Path | None = None, session_id: str | None = None,
                       full_auto: bool = False, catalog=None,
                       model_override_factory=None,
                       extra_tools_factory=None,
                       initial_spec: TeamSpec | None = None) -> SessionRuntime:
    """Open (or resume) a session with real Deep Agents members."""
    from langgraph.checkpoint.sqlite.aio import AsyncSqliteSaver

    from .runners import DeepAgentsRunner

    cwd = (cwd or Path.cwd()).resolve()
    catalog = catalog if catalog is not None else load_user_config(cwd)
    session_id = session_id or default_session_id(cwd)
    paths = session_paths(session_id)
    paths["artifacts"].mkdir(parents=True, exist_ok=True)
    lock_handle = acquire_session_lock(paths)
    # Release the lock on every exit path: a failure halfway through opening
    # a session must not leave a half-open session that looks "already in use".
    stack = contextlib.AsyncExitStack()
    stack.callback(lambda: os.close(lock_handle))
    try:
        store = Store(paths["db"])
        existing = store.get_session(session_id)
        if existing is None:
            mode = PermissionMode.FULL_AUTO if full_auto else permission_mode_from_config(cwd)
            store.create_session(session_id, str(cwd), mode.value)
            spec = initial_spec or default_leader_spec()
            store.save_team_spec(session_id, spec)
            for agent in spec.agents:
                store.ensure_agent(session_id, agent.id)
        spec = store.load_team_spec(session_id)
        if full_auto:
            store.set_permission_mode(session_id, PermissionMode.FULL_AUTO.value)
        policy = PermissionPolicy(mode=PermissionMode(
            store.get_session(session_id)["permissions_mode"]))
        approvals = ApprovalGate(store, session_id, policy)

        saver = await stack.enter_async_context(
            AsyncSqliteSaver.from_conn_string(str(paths["base"] / "checkpoints.sqlite")))
        def make_runner(agent: AgentSpec):
            if agent.runtime_kind is RuntimeKind.CODEX:
                from .codex import CodexRunner
                overrides: dict = {}
                profile = catalog.models.get(agent.model_profile)
                if profile is not None:
                    # one user config drives both backends: profile -> codex -c flags
                    overrides["model"] = profile.model
                    if profile.provider:
                        overrides["model_provider"] = profile.provider
                    for key, value in (profile.generation_options or {}).items():
                        overrides[key] = value
                return CodexRunner(
                    agent=agent, session_id=session_id,
                    workdir=_member_workspace(paths, agent, cwd),
                    approvals=approvals, store=store,
                    sandbox="workspace-write", approval_policy="on-request",
                    effort="xhigh",
                    config_overrides=overrides or None,
                    status_hook=None,    # wired by the runtime below
                    progress_hook=None,
                )
            skills_dirs, memory_files = _skills_and_memory(catalog, cwd, paths, agent)
            return DeepAgentsRunner(
                agent=agent, catalog=catalog, session_id=session_id,
                workdir=_member_workspace(paths, agent, cwd),
                artifacts_dir=paths["artifacts"], checkpointer=saver,
                approvals=approvals, skills_dirs=skills_dirs,
                memory_files=memory_files,
                extra_tools=(extra_tools_factory(agent) if extra_tools_factory else None),
                model_override=(model_override_factory(catalog, agent)
                                if model_override_factory else None),
            )

        runners = {agent.id: make_runner(agent) for agent in spec.agents}
        runtime = SessionRuntime(store, session_id, catalog, runners=runners,
                                 approvals=approvals,
                                 cleanup=lambda: _close_all(stack, runners),
                                 runner_factory=make_runner)
        for runner in runners.values():
            if hasattr(runner, "status_hook"):
                runner.status_hook = runtime.note_external_status
            if hasattr(runner, "progress_hook"):
                runner.progress_hook = runtime.note_external_progress
            if hasattr(runner, "stream_hook"):
                runner.stream_hook = runtime.note_stream_chunk
        return runtime
    except BaseException:
        await stack.aclose()   # also closes the checkpointer opened above
        raise


async def _close_all(stack: contextlib.AsyncExitStack, runners: dict) -> None:
    for runner in runners.values():
        close = getattr(runner, "aclose", None)
        if close is not None:
            try:
                await close()
            except Exception:
                pass
    await stack.aclose()
