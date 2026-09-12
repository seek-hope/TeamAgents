"""AD-1/AD-2/AD-3: reopening a session must not fail, leak its lock, or reuse ids."""

from __future__ import annotations

import asyncio
import subprocess
from pathlib import Path

import pytest

from conftest import leader, member, spec_of
from teamagents import session as session_mod
from teamagents.config import sessions_dir
from teamagents.models import ModelProfile, PermissionMode, UserConfig, WorkspacePolicy
from teamagents.session import open_session
from teamagents.sessions import (archive_session, is_session_locked, list_sessions,
                                 new_session_id)
from teamagents.storage import Store

CATALOG = UserConfig(models={"test": ModelProfile(provider="openai", model="test-model")})


def _scripted_model(catalog, agent):
    from scripted_model import ScriptedChatModel, ai_text
    return ScriptedChatModel(script=[ai_text("ok")])


def git(cwd: Path, *args: str) -> None:
    subprocess.run(["git", "-C", str(cwd), *args], check=True, capture_output=True, text=True)


def make_repo(path: Path) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    git(path, "init", "-q")
    git(path, "config", "user.email", "t@example.com")
    git(path, "config", "user.name", "T")
    (path / "README.md").write_text("hello")
    git(path, "add", "-A")
    git(path, "commit", "-qm", "init")
    return path


def test_reopening_a_worktree_session_reuses_its_worktree(tmp_path, monkeypatch):
    """AD-1: --resume / TUI switch back must not re-run `git worktree add`."""
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    project = make_repo(tmp_path / "project")
    spec = spec_of(leader(), member("b").model_copy(
        update={"workspace_policy": WorkspacePolicy.GIT_WORKTREE}))

    async def scenario() -> Path:
        first = await open_session(cwd=project, session_id="s1", catalog=CATALOG,
                                   initial_spec=spec, model_override_factory=_scripted_model)
        work = sessions_dir() / "s1" / "members" / "b" / "work"
        assert (work / ".git").is_file()
        (work / "result.txt").write_text("member work")     # uncommitted member output
        await first.close()
        first.store.close()

        second = await open_session(cwd=project, session_id="s1", catalog=CATALOG,
                                    initial_spec=spec, model_override_factory=_scripted_model)
        assert (work / "result.txt").read_text() == "member work"
        await second.close()
        second.store.close()
        return work

    work = asyncio.run(scenario())
    listed = subprocess.run(["git", "-C", str(project), "worktree", "list", "--porcelain"],
                            capture_output=True, text=True).stdout
    assert listed.count(f"worktree {work}") == 1


def test_failed_open_session_releases_the_lock(tmp_path, monkeypatch):
    """AD-2: a failure after the lock is taken must not leave a 'running' session."""
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    project = tmp_path / "project"
    project.mkdir()
    spec = spec_of(leader())

    def boom(path):
        raise RuntimeError("injected failure while opening the session")

    with monkeypatch.context() as m:
        m.setattr(session_mod, "Store", boom)
        with pytest.raises(RuntimeError, match="injected failure"):
            asyncio.run(open_session(cwd=project, session_id="s1", catalog=CATALOG,
                                     initial_spec=spec, model_override_factory=_scripted_model))
    assert not is_session_locked("s1"), "failed open still holds the session lock"

    async def retry():
        rt = await open_session(cwd=project, session_id="s1", catalog=CATALOG,
                                initial_spec=spec, model_override_factory=_scripted_model)
        await rt.close()
        rt.store.close()

    asyncio.run(retry())        # must not report SessionInUse
    assert not is_session_locked("s1")


def test_new_session_id_skips_archived_ids(tmp_path, monkeypatch):
    """AD-3: an archived id must not be handed out again (duplicate rows in the TUI)."""
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    project = tmp_path / "project"
    project.mkdir()
    first = new_session_id(project)
    store = Store(sessions_dir() / first / "team.db")
    store.create_session(first, str(project), PermissionMode.APPROVED_SCOPE.value)
    store.close()

    archive_session(first)
    second = new_session_id(project)
    assert second == f"{first}_2", "the archived id must be skipped"

    rows = list_sessions(cwd=project)
    assert [(r.session_id, r.archived) for r in rows] == [(first, True)]
