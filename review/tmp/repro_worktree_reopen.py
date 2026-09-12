"""Repro: reopening a session whose member uses the git_worktree policy.

prepare() creates <member_dir>/work as a worktree. A second prepare() for the
same member directory (what open_session does on --resume / TUI switch back)
runs `git worktree add` at the same path again.
"""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

sys.path.insert(0, "src")
sys.path.insert(0, "tests")

from teamagents.models import RuntimeKind, WorkspacePolicy  # noqa: E402
from teamagents.workspace import prepare  # noqa: E402
from conftest import member  # noqa: E402


def git(cwd: Path, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["git", "-C", str(cwd), *args], capture_output=True, text=True)


def make_repo(path: Path) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    git(path, "init", "-q")
    git(path, "config", "user.email", "t@example.com")
    git(path, "config", "user.name", "T")
    (path / "README.md").write_text("hello")
    git(path, "add", "-A")
    git(path, "commit", "-qm", "init")
    return path


tmp = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/ta-repro-wt")
import shutil  # noqa: E402
shutil.rmtree(tmp, ignore_errors=True)

project = make_repo(tmp / "project")
member_dir = tmp / "members" / "b"
agent = member("b").model_copy(update={"workspace_policy": WorkspacePolicy.GIT_WORKTREE})

ws1 = prepare(agent, project, member_dir)
print("1st prepare ->", ws1.policy, ws1.path, ws1.branch)

# the project itself stays clean; the member's worktree is registered
print("git worktree list:\n" + git(project, "worktree", "list").stdout.strip())

try:
    ws2 = prepare(agent, project, member_dir)
    print("2nd prepare ->", ws2.policy, ws2.path, ws2.branch)
except Exception as e:
    print(f"2nd prepare RAISED {type(e).__name__}: {e}")

print("branches after:", git(project, "branch").stdout.split())

import time  # noqa: E402
time.sleep(1.1)
try:
    ws3 = prepare(agent, project, member_dir)
    print("3rd prepare (1s later) ->", ws3.policy, ws3.path, ws3.branch)
except Exception as e:
    print(f"3rd prepare (1s later) RAISED {type(e).__name__}: {e}")
