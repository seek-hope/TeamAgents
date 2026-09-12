"""Probe: which TUI panels are mounted at startup / after tab switches (textual 8.2.8)."""
import asyncio
import os
import sys
import tempfile
from pathlib import Path

os.environ.setdefault("XDG_STATE_HOME", "/tmp/probe-tui-state")
sys.path.insert(0, "tests")

from conftest import leader, scripts, spec_of  # noqa: E402
from teamagents.runtime import fake_session  # noqa: E402
from teamagents.tui.app import TeamAgentsApp  # noqa: E402
from teamagents.tui.approvals import ApprovalsPanel  # noqa: E402
from teamagents.tui.panels import (LogPanel, SessionsPanel, SettingsPanel,  # noqa: E402
                                   SharedPanel, StatusBar, TasksPanel, TeamPanel)

CLASSES = (TeamPanel, TasksPanel, SharedPanel, ApprovalsPanel, SessionsPanel,
           LogPanel, SettingsPanel)


def dump(app, label):
    print(f"-- {label}: " + ", ".join(
        f"{c.__name__}={len(app.query(c).nodes)}" for c in CLASSES))
    print("   StatusBar:", len(app.query(StatusBar).nodes),
          "| #status type:", type(app.query_one("#status")).__name__)


async def main():
    rt = fake_session(Path(tempfile.mkdtemp()), spec_of(leader()),
                      scripts(leader=[("end",)]))
    await rt.start()
    app = TeamAgentsApp(runtime=rt)
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause(0.6)
        dump(app, "startup (tab-team active)")
        tabs = app.query_one("TabbedContent")
        for tab in ("tab-tasks", "tab-shared", "tab-approvals", "tab-sessions",
                    "tab-log", "tab-settings", "tab-team"):
            tabs.active = tab
            await pilot.pause(0.35)
            dump(app, f"after activate {tab}")
    await rt.close()
    rt.store.close()


asyncio.run(main())
