"""Render the TUI with deterministic data; no credentials or model calls."""
import asyncio
import sys
from pathlib import Path
from tempfile import TemporaryDirectory

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tests"))
from conftest import leader, member, scripts, spec_of, task_channel
from teamagents.runtime import fake_session
from teamagents.models import UserConfig, ModelProfile
from teamagents.tui.app import TeamAgentsApp


async def main():
    with TemporaryDirectory() as tmp:
        spec = spec_of(leader(), member("researcher"), member("coder"),
                       channels=[task_channel("leader", ["researcher", "coder"])])
        rt = fake_session(tmp, spec, scripts(leader=[], researcher=[], coder=[]),
                          catalog=UserConfig(models={"test": ModelProfile(provider="openai", model="test")}))
        app = TeamAgentsApp(runtime=rt)
        async with app.run_test(size=(100, 38)) as pilot:
            await pilot.pause(0.3)
            app._write_chat("你", "审查 Agent 协作问题，并改进终端交互。")
            app._write_chat("Leader", "已完成协作修复，正在验证终端交互。\n\n- 成员独立执行，及时接收补充要求\n- 团队变更在安全边界生效\n- 支持 **多行输入** 和历史草稿恢复")
            app.query_one("#prompt").text = "请继续验证窄屏下的批准与会话切换"
            await pilot.pause(0.3)
            app.save_screenshot("teamagents-tui-wide.svg", path="review")
            await pilot.resize_terminal(70, 26)
            await pilot.pause(0.3)
            app.save_screenshot("teamagents-tui-narrow.svg", path="review")
        await rt.close()
        rt.store.close()


asyncio.run(main())
