"""Workspace policies: shared / isolated / git worktree (plan section 12.3).

A worktree isolates working files; it is not a security sandbox. Uncommitted
task inputs in the original directory are never ignored silently: the policy
falls back to shared mode and says why. Directories with unmerged results,
unresolved conflicts or local modifications are never auto-deleted.
"""

from __future__ import annotations

import dataclasses
import subprocess
import time
from pathlib import Path

from .models import AgentSpec, WorkspacePolicy


@dataclasses.dataclass
class Workspace:
    path: Path
    policy: WorkspacePolicy
    note: str | None = None
    branch: str | None = None
    base_commit: str | None = None


class WorkspaceError(RuntimeError):
    pass


def _git(cwd: Path, *args: str, timeout: int = 30) -> subprocess.CompletedProcess:
    return subprocess.run(["git", "-C", str(cwd), *args], capture_output=True,
                          text=True, timeout=timeout)


def is_git_repo(cwd: Path) -> bool:
    return _git(cwd, "rev-parse", "--git-dir").returncode == 0


def is_dirty(cwd: Path) -> bool:
    result = _git(cwd, "status", "--porcelain")
    return bool(result.stdout.strip())


def head_commit(cwd: Path) -> str | None:
    result = _git(cwd, "rev-parse", "HEAD")
    return result.stdout.strip() if result.returncode == 0 else None


def prepare(agent: AgentSpec, project_cwd: Path, member_dir: Path) -> Workspace:
    """Resolve where this member works, applying the configured policy."""
    policy = agent.workspace_policy
    if policy is WorkspacePolicy.SHARED:
        return Workspace(path=project_cwd, policy=policy)
    if policy is WorkspacePolicy.ISOLATED:
        path = member_dir / "work"
        path.mkdir(parents=True, exist_ok=True)
        (path / "INPUTS.md").touch(exist_ok=True)
        return Workspace(path=path, policy=policy,
                         note="isolated directory: copy inputs explicitly, "
                              "deliver results through artifact refs")
    if not is_git_repo(project_cwd):
        return Workspace(path=project_cwd, policy=WorkspacePolicy.SHARED,
                         note="git_worktree requested but the directory is not a git "
                              "repository; using shared mode")
    if is_dirty(project_cwd):
        return Workspace(path=project_cwd, policy=WorkspacePolicy.SHARED,
                         note="git_worktree requested but the project has uncommitted "
                              "changes; using shared mode so those inputs are not ignored")
    branch = f"teamagents/{agent.id}-{int(time.time())}"
    base = head_commit(project_cwd)
    path = member_dir / "work"
    path.parent.mkdir(parents=True, exist_ok=True)
    result = _git(project_cwd, "worktree", "add", "-b", branch, str(path), base or "HEAD",
                  timeout=120)
    if result.returncode != 0:
        raise WorkspaceError(f"git worktree add failed: {result.stderr.strip()}")
    return Workspace(path=path, policy=policy, branch=branch, base_commit=base)


def cleanup(workspace: Workspace, project_cwd: Path, *, force: bool = False) -> tuple[bool, str]:
    """Remove an isolated/worktree directory only when nothing would be lost."""
    if workspace.policy is WorkspacePolicy.SHARED:
        return False, "shared workspace is never removed"
    if workspace.policy is WorkspacePolicy.GIT_WORKTREE:
        if not force:
            if is_dirty(workspace.path):
                return False, ("worktree has uncommitted or unmerged changes; "
                               "review and merge them first")
            porcelain = _git(workspace.path, "status", "--porcelain").stdout
            if any(line[:2] in ("UU", "AA", "DD", "AU", "UA", "DU", "UD")
                   for line in porcelain.splitlines()):
                return False, "unresolved merge conflicts in the worktree"
            if workspace.branch:
                unmerged = _git(project_cwd, "branch", "--no-merged", "HEAD",
                                "--list", workspace.branch).stdout
                if workspace.branch in unmerged:
                    return False, ("worktree results are unmerged (committed but not "
                                   "merged); merge them before cleanup")
        args = ["worktree", "remove"]
        if force:
            args.append("--force")
        args.append(str(workspace.path))
        result = _git(project_cwd, *args, timeout=120)
        if result.returncode != 0:
            return False, result.stderr.strip()
        if workspace.branch:
            _git(project_cwd, "branch", "-D" if force else "-d", workspace.branch)
        return True, "worktree removed"
    if workspace.policy is WorkspacePolicy.ISOLATED:
        if any(workspace.path.iterdir()) and not force:
            return False, "isolated directory still holds results; archive them first"
        for child in sorted(workspace.path.iterdir(), reverse=True):
            if child.is_dir():
                for sub in sorted(child.rglob("*"), reverse=True):
                    sub.rmdir() if sub.is_dir() else sub.unlink()
                child.rmdir()
            else:
                child.unlink()
        workspace.path.rmdir()
        return True, "isolated directory removed"
    return False, "unknown workspace policy"


def merge_branch(project_cwd: Path, branch: str,
                 message: str | None = None) -> tuple[bool, str]:
    """Leader-side merge helper: merge a member branch, report conflicts."""
    result = _git(project_cwd, "merge", "--no-ff", branch,
                  "-m", message or f"merge {branch}", timeout=120)
    if result.returncode == 0:
        return True, result.stdout.strip()
    if "CONFLICT" in result.stdout or "CONFLICT" in result.stderr:
        return False, "merge conflicts: " + result.stdout.strip()
    return False, result.stderr.strip()
