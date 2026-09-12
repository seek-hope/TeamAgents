"""B-01 probe: run the real `--plain` REPL loop against a real SessionRuntime.

Fake (scripted) members + a real SQLite event log: no provider key needed. The fake
session is built inside the REPL's own event loop (`open_session` is replaced for the
probe), one Leader reply is emitted through the normal team-write path, one user line
is fed in, then EOF.

Before the fix: `AttributeError: 'SessionRuntime' object has no attribute 'ui_cursor'`
and the Leader reply is never printed. After the fix: the reply line is printed.
"""
from __future__ import annotations

import builtins
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, "src")
sys.path.insert(0, "tests")

from conftest import leader, scripts, spec_of                       # noqa: E402
from teamagents import cli, session as session_mod                  # noqa: E402
from teamagents.control import EventDraft                           # noqa: E402
from teamagents.models import EventKind                             # noqa: E402
from teamagents.runtime import fake_session                         # noqa: E402

TMP = Path(tempfile.mkdtemp(prefix="plain-repl-"))
REPLY = "计划已就位：先做 A，再做 B"


async def open_fake(**kwargs):
    rt = fake_session(TMP, spec_of(leader()), scripts(leader=[("end",)]), session_id="s1")
    print("hasattr(rt, 'ui_cursor') ->", hasattr(rt, "ui_cursor"), flush=True)
    await rt.start()
    rt.control.emit([EventDraft(kind=EventKind.LEADER_REPLY, payload={"text": REPLY})],
                    actor_id="leader")
    return rt


session_mod.open_session = open_fake
_lines = iter(["请开始"])


def fake_input(prompt: str = "") -> str:
    try:
        return next(_lines)
    except StopIteration as e:
        raise EOFError from e


builtins.input = fake_input
try:
    print("cli.main returned", cli.main(["--plain", "--cwd", str(TMP)]))
except Exception as e:
    print(f"REPL RAISED {type(e).__name__}: {e}")
