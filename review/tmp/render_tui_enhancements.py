"""Render seeded UI states without executing models or changing user preferences."""
import asyncio
import os
import sys
import time
from pathlib import Path
from tempfile import TemporaryDirectory

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tests"))
from conftest import leader, member, scripts, spec_of
from teamagents.models import ModelProfile, Task, TurnRun, UserConfig
from teamagents.runtime import fake_session
from teamagents.tui.app import TeamAgentsApp
from textual.widgets import Select, TabbedContent


async def main():
    with TemporaryDirectory() as tmp:
        os.environ["XDG_STATE_HOME"] = tmp
        rt = fake_session(tmp, spec_of(leader(), member("researcher"), member("coder")),
                          scripts(leader=[], researcher=[], coder=[]),
                          catalog=UserConfig(models={"test": ModelProfile(provider="openai", model="test")}))
        app = TeamAgentsApp(runtime=rt)
        async with app.run_test(size=(120, 40)) as pilot:
            await pilot.pause(0.2)
            for task_id, status, offset, description in [
                ("task-oldest", "BLOCKED", 180, "Check the integration environment"),
                ("task-middle", "SUCCEEDED", 120, "Inspect collaboration events"),
                ("task-latest", "RUNNING", 35, "Verify the updated terminal UI")]:
                rt.store.insert_task("s1", Task(task_id=task_id, status=status, assignee="coder",
                    requester="leader", description=description, created_at=time.time()-offset))
            rt.store.insert_run(TurnRun(run_id="preview-run", session_id="s1", agent_id="coder",
                task_id="task-latest", config_revision=1, topology_revision=1,
                status="RUNNING", created_at=time.time()-35))
            app._write_chat("user", "Sort tasks by time and make team activity visible.")
            app._write_chat("Leader", "I am checking the interface.\n\n- **Newest tasks first**, regardless of status.\n- Language and animation controls are in **Settings**.\n- Team activity stays visible while members work.")
            app._latest_activity = ("{agent} 开始处理", {"agent": "coder"})
            app.query_one(TabbedContent).active = "tab-tasks"
            await app._refresh_widgets()
            await pilot.pause(0.3)
            app.save_screenshot("tui-tasks-english.svg", path="review")
            app.query_one(TabbedContent).active = "tab-settings"
            await pilot.pause(0.2)
            app.query_one("#language-select", Select).value = "zh-CN"
            await pilot.pause(0.3)
            app.save_screenshot("tui-settings-chinese.svg", path="review")
            app.query_one("#language-select", Select).value = "en"
            app.query_one(TabbedContent).active = "tab-team"
            rt.store.set_run_status("preview-run", "WAITING_APPROVAL")
            app._latest_activity = ("需要用户批准", {})
            await app._refresh_widgets()
            await pilot.resize_terminal(80, 30)
            await pilot.pause(0.3)
            app.save_screenshot("tui-waiting-english.svg", path="review")
        await rt.close()
        rt.store.close()


asyncio.run(main())
