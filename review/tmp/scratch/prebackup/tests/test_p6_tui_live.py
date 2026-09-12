"""P6 live: the TUI drives a real model session end to end."""

from __future__ import annotations

import os

import pytest

from teamagents.models import ModelProfile, UserConfig
from teamagents.session import open_session
from teamagents.tui.app import TeamAgentsApp
from teamagents.tui.panels import PromptInput

pytestmark = [pytest.mark.live, pytest.mark.skipif(
    not os.environ.get("DEEPSEEK_API_KEY"), reason="DEEPSEEK_API_KEY not set")]

DEEPSEEK = ModelProfile(provider="deepseek", protocol="deepseek",
                        model="deepseek-flash", api_key_env="DEEPSEEK_API_KEY",
                        timeout=120, max_retries=1,
                        generation_options={"reasoning_effort": "high"})


async def test_tui_live_session_shows_leader_reply(tmp_path, monkeypatch):
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    project = tmp_path / "project"
    project.mkdir()
    rt = await open_session(cwd=project, session_id="tui-live",
                            catalog=UserConfig(models={"leader_main": DEEPSEEK}))
    app = TeamAgentsApp(runtime=rt)
    try:
        async with app.run_test(size=(120, 40)) as pilot:
            await pilot.pause(0.3)
            prompt = app.query_one("#prompt", PromptInput)
            prompt.text = ("请用一句话回答：1+1 等于几？然后调用 signal_done，"
                           "summary 写你的答案。")
            await pilot.press("enter")
            deadline = 0
            while deadline < 240:
                await pilot.pause(1.0)
                deadline += 1
                if rt.store.get_session("tui-live")["goal_state"] == "done":
                    break
            await pilot.pause(0.6)
            assert rt.store.get_session("tui-live")["goal_state"] == "done", \
                "真实会话应完成目标"
            kinds = [e["kind"] for e in rt.store.events("tui-live")]
            assert "leader_reply" in kinds, "Leader 回复要落成事件供界面呈现"
            chat = app.query_one("#chat-stream")
            text = "\n".join(str(line) for line in chat.lines)
            assert "2" in text, "对话视图要显示 Leader 的答案"
    finally:
        await rt.close()
        rt.store.close()
