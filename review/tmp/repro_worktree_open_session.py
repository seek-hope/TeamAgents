"""Repro: resuming a session whose member uses git_worktree fails in open_session."""
from __future__ import annotations

import asyncio, os, shutil, subprocess, sys
from pathlib import Path

sys.path.insert(0, "src"); sys.path.insert(0, "tests")
ROOT = Path("review/tmp/scratch/wt-open").resolve()
shutil.rmtree(ROOT, ignore_errors=True)
os.environ["XDG_STATE_HOME"] = str(ROOT / "state")
PROJECT = ROOT / "project"; PROJECT.mkdir(parents=True)

def git(*args):
    r = subprocess.run(["git", "-C", str(PROJECT), *args], capture_output=True, text=True)
    assert r.returncode == 0, r.stderr
    return r.stdout

git("init", "-q"); git("config", "user.email", "t@e.com"); git("config", "user.name", "T")
(PROJECT / "README.md").write_text("hello"); git("add", "-A"); git("commit", "-qm", "init")

from teamagents.models import (AgentSpec, ModelProfile, RuntimeKind, TeamSpec,
                               UserConfig, WorkspacePolicy)
from teamagents.session import open_session
from teamagents.workspace import is_dirty

SPEC = TeamSpec(leader_id="leader", agents=[
    AgentSpec(id="leader", name="L", role="leader", runtime_kind=RuntimeKind.DEEPAGENTS,
              instructions="lead", model_profile="test", tool_bindings=["files"]),
    AgentSpec(id="b", name="B", role="worker", runtime_kind=RuntimeKind.DEEPAGENTS,
              instructions="work", model_profile="test", tool_bindings=["files"],
              workspace_policy=WorkspacePolicy.GIT_WORKTREE)], shared_spaces=[])
CATALOG = UserConfig(models={"test": ModelProfile(provider="openai", model="test")})

def scripted(catalog, agent):
    from scripted_model import ScriptedChatModel, ai_text
    return ScriptedChatModel(script=[ai_text("ok")])

async def main():
    print("project dirty before 1st open:", is_dirty(PROJECT), flush=True)
    first = await open_session(cwd=PROJECT, session_id="s1", catalog=CATALOG,
                               initial_spec=SPEC, model_override_factory=scripted)
    print("1st open OK; dirty now:", is_dirty(PROJECT), flush=True)
    print("git status:", git("status", "--porcelain").strip(), flush=True)
    await first.close(); first.store.close()
    try:
        second = await open_session(cwd=PROJECT, session_id="s1", catalog=CATALOG,
                                    initial_spec=SPEC, model_override_factory=scripted)
        print("2nd open OK:", second, flush=True)
    except Exception as e:
        print(f"2nd open RAISED {type(e).__name__}: {e}", flush=True)

asyncio.run(main())
