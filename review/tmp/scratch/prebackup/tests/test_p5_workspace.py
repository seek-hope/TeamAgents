"""P5/T18: workspace policies — shared, isolated, git worktree, dirty-input
handling, and no-deletion-without-mercy rules."""

from __future__ import annotations

import subprocess
from pathlib import Path

from conftest import leader, member, spec_of
from teamagents.models import WorkspacePolicy
from teamagents.workspace import cleanup, is_dirty, merge_branch, prepare


def git(cwd: Path, *args: str) -> None:
    subprocess.run(["git", "-C", str(cwd), *args], check=True, capture_output=True,
                   text=True)


def make_repo(path: Path) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    git(path, "init", "-q")
    git(path, "config", "user.email", "t@example.com")
    git(path, "config", "user.name", "T")
    (path / "README.md").write_text("hello")
    git(path, "add", "-A")
    git(path, "commit", "-qm", "init")
    return path


def test_shared_policy_uses_project_directory(tmp_path):
    project = make_repo(tmp_path / "proj")
    agent = leader()
    ws = prepare(agent, project, tmp_path / "member")
    assert ws.path == project and ws.policy is WorkspacePolicy.SHARED
    assert ws.note is None


def test_isolated_policy_creates_member_directory(tmp_path):
    project = make_repo(tmp_path / "proj")
    agent = member("b").model_copy(update={"workspace_policy": WorkspacePolicy.ISOLATED})
    ws = prepare(agent, project, tmp_path / "member")
    assert ws.path == tmp_path / "member" / "work"
    assert (ws.path / "INPUTS.md").exists()
    assert "isolated" in (ws.note or "")


def test_git_worktree_creates_branch_from_head(tmp_path):
    project = make_repo(tmp_path / "proj")
    (project / "wip.txt").write_text("x")
    git(project, "add", "-A")
    git(project, "commit", "-qm", "wip")
    agent = member("b").model_copy(
        update={"workspace_policy": WorkspacePolicy.GIT_WORKTREE})
    ws = prepare(agent, project, tmp_path / "member")
    assert ws.policy is WorkspacePolicy.GIT_WORKTREE
    assert ws.path.is_dir() and ws.branch and ws.base_commit
    branches = subprocess.run(["git", "-C", str(project), "branch"],
                              capture_output=True, text=True).stdout
    assert ws.branch in branches
    # the original directory shows uncommitted inputs back to the caller
    assert not is_dirty(project)


def test_dirty_project_falls_back_to_shared_with_explanation(tmp_path):
    project = make_repo(tmp_path / "proj")
    (project / "uncommitted.txt").write_text("user work in progress")
    agent = member("b").model_copy(
        update={"workspace_policy": WorkspacePolicy.GIT_WORKTREE})
    ws = prepare(agent, project, tmp_path / "member")
    assert ws.policy is WorkspacePolicy.SHARED and ws.path == project
    assert "uncommitted" in (ws.note or "")
    assert (project / "uncommitted.txt").read_text() == "user work in progress"


def test_worktree_cleanup_refuses_until_merged(tmp_path):
    project = make_repo(tmp_path / "proj")
    agent = member("b").model_copy(
        update={"workspace_policy": WorkspacePolicy.GIT_WORKTREE})
    ws = prepare(agent, project, tmp_path / "member")
    (ws.path / "result.txt").write_text("member output")
    ok, reason = cleanup(ws, project)
    assert not ok and "uncommitted" in reason
    assert (ws.path / "result.txt").exists()

    git(ws.path, "add", "-A")
    git(ws.path, "commit", "-qm", "member result")
    ok, reason = cleanup(ws, project)
    assert not ok and "merged" in reason, "committed but unmerged work must be kept"

    merged, message = merge_branch(project, ws.branch)
    assert merged, message
    ok, reason = cleanup(ws, project)
    assert ok, reason
    assert not ws.path.exists()
    assert (project / "result.txt").exists()


def test_team_spec_accepts_all_workspace_policies():
    for policy in WorkspacePolicy:
        spec = spec_of(leader(),
                       member("b").model_copy(update={"workspace_policy": policy}))
        assert spec.agent("b").workspace_policy is policy
