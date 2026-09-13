"""Dump specific Python TUI widget regions as text (widget-level parity check)."""
import asyncio, sys
from pathlib import Path
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tests"))
sys.path.insert(0, str(ROOT / "src"))
from conftest import leader, member, scripts, spec_of, task_channel, msg_channel
from teamagents.runtime import fake_session
from teamagents.models import UserConfig, ModelProfile
from teamagents.tui.app import TeamAgentsApp


def region(screen) -> list[str]:
    comp = screen._compositor
    out = []
    for strip in comp.render_strips():
        out.append("".join(seg.text for seg in getattr(strip, "_segments", [])).rstrip())
    return out


async def main():
    with TemporaryDirectory() as tmp:
        spec = spec_of(leader(), member("researcher"), member("coder"),
                       channels=[task_channel("leader", ["researcher", "coder"]),
                                 msg_channel("researcher", ["leader"]), msg_channel("coder", ["leader"])],
                       shared_spaces=[{"id": "main", "readers": ["leader", "researcher", "coder"],
                                       "writers": ["leader", "researcher"]}])
        rt = fake_session(tmp, spec, scripts(leader=[], researcher=[], coder=[]),
                          catalog=UserConfig(models={"test": ModelProfile(provider="openai", model="test")}))
        app = TeamAgentsApp(runtime=rt)
        async with app.run_test(size=(200, 44)) as pilot:
            await pilot.pause(0.3)
            app._write_chat("You", "审查 Agent 协作问题，并改进终端交互。")
            app._write_chat("Leader", "已完成协作修复，正在验证终端交互。\n\n- 成员独立执行\n- 支持 **多行输入**")
            app._write_chat("researcher→leader", "调研完成：三个候选方案")
            app.query_one("#prompt").text = "请继续验证窄屏下的批准与会话切换"
            await pilot.pause(0.4)
            rows = region(app.screen)
            footer = app.query_one("Footer")
            print("== FRAME 200x44 ==")
            for i, line in enumerate(rows):
                print(f"{i:02d}|{line}")
            print("== footer widget:", repr(str(footer.render_line(0).text)))
            print("== status:", repr(app.query_one("#status").renderable if hasattr(app.query_one("#status"), "renderable") else app.query_one("#status")._content))
        await rt.close()
        rt.store.close()

asyncio.run(main())
