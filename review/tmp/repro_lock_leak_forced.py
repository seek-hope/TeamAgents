"""AD-2 probe: force a failure after the lock is taken and check it is released.

The original repros (repro_lock_leak.py / repro_open_session_lock_leak.py) used the
repeated-``worktree add`` failure as the trigger; AD-1 now makes that path succeed,
so this probe injects the failure explicitly (Store raises once) to keep testing
AD-2 on its own.
"""
from __future__ import annotations

import asyncio
import os
import shutil
import sys
from pathlib import Path

sys.path.insert(0, "src")

ROOT = Path("review/tmp/scratch/lockleak-forced").resolve()
shutil.rmtree(ROOT, ignore_errors=True)
os.environ["XDG_STATE_HOME"] = str(ROOT / "state")
PROJECT = ROOT / "project"
PROJECT.mkdir(parents=True)

from teamagents.models import (  # noqa: E402
    AgentSpec, ModelProfile, RuntimeKind, TeamSpec, UserConfig,
)
from teamagents import session as session_mod  # noqa: E402
from teamagents.session import open_session  # noqa: E402
from teamagents.sessions import is_session_locked  # noqa: E402

SPEC = TeamSpec(leader_id="leader", agents=[
    AgentSpec(id="leader", name="L", role="leader", runtime_kind=RuntimeKind.DEEPAGENTS,
              instructions="lead", model_profile="test", tool_bindings=["files"])],
    shared_spaces=[])
CATALOG = UserConfig(models={"test": ModelProfile(provider="openai", model="test")})


def lock_fds() -> list[str]:
    return [f for f in os.listdir("/proc/self/fd")
            if os.path.exists("/proc/self/fd/" + f)
            and "session.lock" in os.readlink("/proc/self/fd/" + f)]


async def main() -> None:
    real_store = session_mod.Store

    class Boom(real_store):  # type: ignore[misc, valid-type]
        def __init__(self, path):
            raise RuntimeError("injected failure while opening the session")

    session_mod.Store = Boom
    try:
        await open_session(cwd=PROJECT, session_id="s1", catalog=CATALOG,
                           initial_spec=SPEC)
        print("FAILED: open_session unexpectedly succeeded", flush=True)
    except RuntimeError as e:
        print(f"open_session RAISED RuntimeError: {e}", flush=True)
    finally:
        session_mod.Store = real_store

    print("after FAILED open: locked =", is_session_locked("s1"),
          "lock fds:", lock_fds(), flush=True)
    try:
        rt = await open_session(cwd=PROJECT, session_id="s1", catalog=CATALOG,
                                initial_spec=SPEC)
        print("retry open OK:", rt.session_id, flush=True)
        await rt.close()
        rt.store.close()
    except Exception as e:
        print(f"retry RAISED {type(e).__name__}: {str(e)[:90]}", flush=True)
    print("after clean close: locked =", is_session_locked("s1"),
          "lock fds:", lock_fds(), flush=True)


asyncio.run(main())
