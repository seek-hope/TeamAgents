"""impl_sessions fix patch: B-01 (cli), AD-1 (workspace), AD-2 (session), AD-3 (sessions).

Idempotency is *not* the goal: every anchor is asserted to appear exactly once, so
re-running this on already-patched sources fails loudly instead of corrupting them.
"""
from __future__ import annotations

import pathlib
import sys


def patch(rel: str, old: str, new: str) -> None:
    p = pathlib.Path(rel)
    s = p.read_text()
    assert s.count(old) == 1, f"anchor not found exactly once in {rel}: {s.count(old)}"
    p.write_text(s.replace(old, new))
    print("patched", rel)


# --- AD-1: git_worktree member reopened (resume / TUI switch back) -------------
patch("src/teamagents/workspace.py", '''    branch = f"teamagents/{agent.id}-{int(time.time())}"
    base = head_commit(project_cwd)
    path = member_dir / "work"
    path.parent.mkdir(parents=True, exist_ok=True)
    result = _git(project_cwd, "worktree", "add", "-b", branch, str(path), base or "HEAD",
                  timeout=120)
    if result.returncode != 0:
        raise WorkspaceError(f"git worktree add failed: {result.stderr.strip()}")
    return Workspace(path=path, policy=policy, branch=branch, base_commit=base)
''', '''    path = member_dir / "work"
    base = head_commit(project_cwd)
    if (path / ".git").is_file():
        # Reopening a session (--resume, TUI switch back, restart) must reuse the
        # worktree this member already owns: a second `worktree add` always fails
        # and would also hide the member's uncommitted work.
        head = _git(path, "rev-parse", "--abbrev-ref", "HEAD").stdout.strip()
        branch = head if head and head != "HEAD" else None
        merged = _git(project_cwd, "merge-base", "HEAD", branch or "HEAD")
        return Workspace(path=path, policy=policy, branch=branch,
                         base_commit=merged.stdout.strip() or base)
    if path.exists():
        # Never silently adopt (or overwrite) a directory that is not the worktree
        # we would have created; say so and let the user archive/remove it.
        raise WorkspaceError(
            f"{path} exists but is not a git worktree (no .git file); archive or "
            "remove it before starting a new worktree")
    branch = f"teamagents/{agent.id}-{int(time.time())}"
    path.parent.mkdir(parents=True, exist_ok=True)
    if _git(project_cwd, "rev-parse", "--verify", "--quiet",
            f"refs/heads/{branch}").returncode == 0:
        # branch left behind by a crashed or manually removed worktree: re-attach it
        add = ["worktree", "add", str(path), branch]
    else:
        add = ["worktree", "add", "-b", branch, str(path), base or "HEAD"]
    result = _git(project_cwd, *add, timeout=120)
    if result.returncode != 0:
        raise WorkspaceError(f"git worktree add failed: {result.stderr.strip()}")
    return Workspace(path=path, policy=policy, branch=branch, base_commit=base)
''')

# --- AD-2: release the session lock on every exit path -------------------------
p = pathlib.Path("src/teamagents/session.py")
s = p.read_text()
anchor = "    lock_handle = acquire_session_lock(paths)\n"
start = s.index(anchor)
end = s.index("    return runtime\n", start)
body = s[start + len(anchor):end]
moved = "    stack = contextlib.AsyncExitStack()\n    stack.callback(lambda: os.close(lock_handle))\n"
assert moved in body, "stack registration not found in body"
body = body.replace(moved, "").lstrip("\n")
indented = "".join(("    " + line if line.strip() else line)
                   for line in body.splitlines(keepends=True))
p.write_text(s[:start] + anchor + (
    "    # Release the lock on every exit path: a failure halfway through opening\n"
    "    # a session must not leave a half-open session that looks \"already in use\".\n"
    "    stack = contextlib.AsyncExitStack()\n"
    "    stack.callback(lambda: os.close(lock_handle))\n"
    "    try:\n"
) + indented + (
    "        return runtime\n"
    "    except BaseException:\n"
    "        await stack.aclose()   # also closes the checkpointer opened above\n"
    "        raise\n"
) + s[end + len("    return runtime\n"):])
print("patched", p)

# --- AD-3: never hand out an id that an archived session already used ----------
patch("src/teamagents/sessions.py", '''    root = sessions_dir()
    existing = {p.name for p in root.iterdir()} if root.is_dir() else set()
    base = default_session_id(cwd)
''', '''    root = sessions_dir()
    # Archived ids are gone from the active directory but must not be reused: the
    # inventory lists both groups, and a duplicate id makes the TUI row key
    # ambiguous (duplicate keys) and the wrong record easy to switch to/delete.
    existing: set[str] = set()
    for group in (root, root / "archived"):
        if group.is_dir():
            existing.update(p.name for p in group.iterdir())
    base = default_session_id(cwd)
''')

# --- B-01: --plain REPL used a cursor that does not exist on SessionRuntime ----
patch("src/teamagents/cli.py", '''        await rt.start()
        try:
            while True:
                try:
                    line = await asyncio.to_thread(input, "you> ")
                except EOFError:
                    break
                if not line.strip():
                    continue
                receipt = rt.user_message(line)
                print(f"  [input received: {receipt.result.get('goal_id', '')}]")
                await rt.settle(timeout=600)
                for event in rt.store.events(rt.session_id, after_sequence=rt.ui_cursor):
                    rt.ui_cursor = event["sequence"]
                    _print_event(event)
''', '''        await rt.start()
        cursor = 0      # local event cursor for this REPL (no runtime-side cursor)
        try:
            while True:
                try:
                    line = await asyncio.to_thread(input, "you> ")
                except EOFError:
                    break
                if not line.strip():
                    continue
                receipt = rt.user_message(line)
                print(f"  [input received: {receipt.result.get('goal_id', '')}]")
                await rt.settle(timeout=600)
                for event in rt.store.events(rt.session_id, after_sequence=cursor):
                    cursor = event["sequence"]
                    _print_event(event)
''')

patch("src/teamagents/cli.py", '''    elif kind == "approval_requested":
        print(f"  [approval] {json.dumps(payload, ensure_ascii=False)[:200]}")
''', '''    elif kind == "approval_requested":
        print(f"  [approval] {json.dumps(payload, ensure_ascii=False)[:200]}")
    elif kind == "leader_reply":
        print(f"  [Leader] {payload.get('text', '')[:2000]}")
    elif kind == "run_failed":
        error = str(payload.get("error") or "未知错误")[:400]
        print(f"  [运行失败] {actor}: {error}")
''')

print("all patches applied")
