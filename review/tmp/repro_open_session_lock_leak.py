"""Repro: a failed open_session leaves the session lock held (same process).

Scenario: a session whose member uses git_worktree. The first open_session
succeeds; a second open_session (resume / TUI switch back) fails inside
git worktree add, and the file lock taken at the start of open_session is
never released, so every later attempt in the same process reports
"session is already running in another process".
"""
from __future__ import annotations

import asyncio
import os
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, "src")
sys.path.insert(0, "tests")

os.environ.setdefault("XDG_STATE_HOME", "review/tmp/scratch/lockrepro/state")

from teamagents.models import (  # noqa: E402
    AgentSpec, ModelProfile, RuntimeKind, TeamSpec, UserConfig, WorkspacePolicy,
)
from teamagents.session import open_session  # noqa: E402

ROOT = Path("review/tmp/scratch/lockrepro")
shutil.rmtree(ROOT, ignore_errors=True)
PROJECT = ROOT / "project"
PROJECT.mkdir(parents=True)


def git(*args: str) -> None:
    subprocess.run(["git", "-C", str(PROJECT), *args], check=True, capture_output=True)


git("init", "-q")
git("config", "user.email", "t@example.com")
git("config", "user.name", "T")
(PROJECT / "README.md").write_text("hello")
git("add", "-A")
git("commit", "-qm", "init")

SPEC = TeamSpec(
    leader_id="leader",
    agents=[
        AgentSpec(id="leader", name="Leader", role="leader",
                  runtime_kind=RuntimeKind.DEEPAGENTS, instructions="lead",
                  model_profile="test", tool_bindings=["files"]),
        AgentSpec(id="b", name="B", role="worker", runtime_kind=RuntimeKind.DEEPAGENTS,
                  instructions="work", model_profile="test", tool_bindings=["files"],
                  workspace_policy=WorkspacePolicy.GIT_WORKTREE),
    ],
    shared_spaces=[],
)
CATALOG = UserConfig(models={"test": ModelProfile(provider="openai", model="test")})


def scripted(catalog, agent):
    from scripted_model import ScriptedChatModel, ai_text
    return ScriptedChatModel(script=[ai_text("ok")])


async def main() -> None:
    first = await open_session(cwd=PROJECT, session_id="s1", catalog=CATALOG,
                               initial_spec=SPEC, model_override_factory=scripted)
    print("first open ok; lock held by us")
    await first.close()
    first.store.close()
    print("after close, is_session_locked:", end=" ")
    from teamagents.sessions import is_session_locked
    print(is_session_locked("s1"))

    try:
        second = await open_session(cwd=PROJECT, session_id="s1", catalog=CATALOG,
                                    initial_spec=SPEC, model_override_factory=scripted)
        print("second open surprisingly ok:", second)
    except Exception as e:
        print(f"second open RAISED {type(e).__name__}: {e}")
    print("is_session_locked after failed open:", is_session_locked("s1"))

    try:
        third = await open_session(cwd=PROJECT, session_id="s1", catalog=CATALOG,
                                   initial_spec=SPEC, model_override_factory=scripted)
        print("third open surprisingly ok:", third)
    except Exception as e:
        print(f"third open RAISED {type(e).__name__}: {e}")


asyncio.run(main())
