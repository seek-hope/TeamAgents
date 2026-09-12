"""P5/T18: workspace policies — shared, isolated, git worktree, dirty-input
handling, and no-deletion-without-mercy rules."""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

from conftest import leader, member, spec_of
from teamagents.models import WorkspacePolicy
from teamagents.workspace import WorkspaceError, cleanup, is_dirty, merge_branch, prepare


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


def test_worktree_prepare_reuses_the_existing_worktree(tmp_path):
    """AD-1: --resume / TUI switch back must reuse the member's worktree."""
    project = make_repo(tmp_path / "proj")
    agent = member("b").model_copy(
        update={"workspace_policy": WorkspacePolicy.GIT_WORKTREE})
    member_dir = tmp_path / "member"

    first = prepare(agent, project, member_dir)
    (first.path / "result.txt").write_text("member work")   # uncommitted member output

    second = prepare(agent, project, member_dir)
    assert second.policy is WorkspacePolicy.GIT_WORKTREE
    assert second.path == first.path
    assert second.branch == first.branch
    assert (second.path / "result.txt").read_text() == "member work"
    listed = subprocess.run(["git", "-C", str(project), "worktree", "list", "--porcelain"],
                            capture_output=True, text=True).stdout
    assert listed.count(f"worktree {second.path}") == 1, "no second worktree for one member"


def test_worktree_prepare_refuses_a_foreign_existing_path(tmp_path):
    """AD-1: an existing non-worktree directory is reported, never adopted silently."""
    project = make_repo(tmp_path / "proj")
    agent = member("b").model_copy(
        update={"workspace_policy": WorkspacePolicy.GIT_WORKTREE})
    work = tmp_path / "member" / "work"
    work.mkdir(parents=True)
    (work / "notes.txt").write_text("not a worktree")

    with pytest.raises(WorkspaceError, match="not a git worktree"):
        prepare(agent, project, tmp_path / "member")
    assert (work / "notes.txt").read_text() == "not a worktree", "nothing may be destroyed"


def test_worktree_prepare_after_cleanup_starts_fresh(tmp_path):
    """AD-1: after a clean cleanup the member gets a worktree again."""
    project = make_repo(tmp_path / "proj")
    agent = member("b").model_copy(
        update={"workspace_policy": WorkspacePolicy.GIT_WORKTREE})
    member_dir = tmp_path / "member"
    first = prepare(agent, project, member_dir)
    ok, reason = cleanup(first, project)
    assert ok, reason

    second = prepare(agent, project, member_dir)
    assert second.path == first.path and (second.path / ".git").is_file()
