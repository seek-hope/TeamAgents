"""P6/T20: the TUI stays usable while members work — input, navigation,
approvals, streaming refresh, narrow terminals and multi-line Chinese input."""

from __future__ import annotations

import asyncio
import json
import time

import pytest

from conftest import Harness, leader, member, msg_channel, scripts, spec_of, task_channel
from teamagents.models import ApprovalRequest, ApprovalStatus, TeamAction
from teamagents.tui.app import TeamAgentsApp
from teamagents.tui.approvals import ApprovalsPanel
from teamagents.tui.panels import ChatLog, PromptInput, TeamPanel


async def _session(harness_factory, *, busy: bool = False):
    spec = spec_of(leader(), member("b"),
                   channels=[task_channel("leader", ["b"]), msg_channel("b", ["leader"])])
    if busy:
        members = scripts(
            leader=[("call", "assign_task", {"assignee": "b", "description": "slow"}),
                    ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}),
                    ("wait",), ("call", "signal_done", {}), ("end",)],
            b=[("sleep", 1.2),
               ("call", "complete_task", {"task_id": "$inbox0.payload.task_id"}),
               ("end",)],
        )
    else:
        members = scripts(leader=[("call", "signal_done", {"summary": "hi"}),
                                  ("end",)],
                          b=[("end",)])
    h = await harness_factory(spec, members)
    return h


async def test_tui_accepts_multiline_chinese_input_and_shows_reply(harness_factory):
    h = await _session(harness_factory)
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause()
        prompt = app.query_one("#prompt", PromptInput)
        prompt.text = "第一行：请总结\n第二行：包含中文"
        await pilot.press("enter")
        await pilot.pause(0.3)
        await h.settle(10)
        await pilot.pause(0.4)

        messages = [e for e in h.rt.store.events("s1")
                    if e["kind"] == "user_message"]
        assert messages, "UI 输入必须进入团队事务"
        payload = json.loads(messages[0]["payload_json"])
        assert "第一行" in payload["text"] and "第二行" in payload["text"]
        chat = app.query_one("#chat-stream")
        text = "\n".join(str(line) for line in chat.lines)
        assert "第一行" in text, "对话视图要显示用户输入"
        assert "目标完成" in text or "hi" in text, "对话视图要显示 Leader 结果"


async def test_tui_input_and_navigation_work_while_members_run(harness_factory):
    h = await _session(harness_factory, busy=True)
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause()
        await h.user("开始慢任务")
        deadline = time.time() + 5
        while not [r for r in h.rt.store.runs_for_session("s1", ["RUNNING"])
                   if r.agent_id == "b"]:
            assert time.time() < deadline
            await asyncio.sleep(0.01)
        # 成员执行期间：输入、切换面板、刷新都要可用
        prompt = app.query_one("#prompt", PromptInput)
        prompt.text = "执行期间补充一句"
        await pilot.press("enter")
        await pilot.press("ctrl+t")           # cycle panel
        await pilot.press("ctrl+r")           # refresh views
        await pilot.press("ctrl+t")
        await pilot.pause(0.2)
        supplements = [e for e in h.rt.store.events("s1")
                       if e["kind"] == "user_message"
                       and "补充" in json.loads(e["payload_json"])["text"]]
        assert supplements, "成员执行期间输入必须被接收"
        # 视图仍然可读
        await pilot.pause(0.5)
        team_rows = app.query_one("#team-table").row_count
        assert team_rows == 2, f"团队面板应列出成员，实际 {team_rows}"
        await h.settle(15)


async def test_tui_streaming_deltas_are_coalesced_into_chat(harness_factory):
    h = await _session(harness_factory)
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause()
        log = app.query_one("#chat-stream")
        before = len(log.lines)
        for chunk in ["流式", "输出", "测试"]:
            h.rt.note_stream_chunk("run-x", "leader", chunk)
        await pilot.pause(0.3)               # coalesced refresh interval
        assert len(log.lines) == before, "流式预览不能反复追加到历史"
        live = app.query_one("#chat-live")
        assert live.display
        assert live.content.markup == "流式输出测试"
        for _ in range(100):
            h.rt.note_stream_chunk("run-x", "leader", "中" * 1000)
        await pilot.pause(0.2)
        assert len(app._delta_buffer["run-x"]) <= 32000


async def test_tui_approval_queue_actions(harness_factory):
    h = await _session(harness_factory)
    approval = ApprovalRequest(
        approval_id="appr-ui-1", session_id="s1", agent_id="b", run_id="run-x",
        tool_call_id="call-1", operation_hash="hash-ui",
        requested_scope={"tool": "shell", "args": {"command": "rm -rf /tmp/x"}},
        policy_revision=1)
    with h.rt.store.tx():
        h.rt.store.insert_approval(approval)
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause()
        await pilot.press("ctrl+g")          # jump to approvals
        await pilot.pause(0.6)
        panel = app.query_one(ApprovalsPanel)
        table = panel.query_one("#approvals-table")
        assert table.row_count == 1
        table.focus()
        await pilot.pause(0.1)
        await pilot.press("s")               # session-wide approval
        await pilot.pause(0.3)
        stored = h.rt.store.get_approval("appr-ui-1")
        assert stored.status == ApprovalStatus.APPROVED_SESSION


@pytest.mark.parametrize("size", [(120, 40), (70, 30), (80, 24)])
async def test_tui_vertical_layout_keeps_both_sections_accessible(harness_factory, size):
    h = await _session(harness_factory)
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=size) as pilot:
        await pilot.pause()
        top, bottom = app.query_one("#side"), app.query_one("#chat")
        prompt = app.query_one("#prompt")
        assert top.display and bottom.display
        assert top.region.bottom <= bottom.region.y
        assert top.region.width == bottom.region.width
        assert prompt.region.bottom <= size[1] - 1
        await pilot.press("ctrl+g")
        await pilot.pause(0.3)
        assert app.query_one("TabbedContent").active == "tab-approvals"


async def test_tui_settings_panel_and_full_auto_toggle(harness_factory):
    """设置视图：会话/权限/上限/目录可见，且全自动只能由用户在这里开。"""
    h = await _session(harness_factory)
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause(0.3)
        tabs = app.query_one("TabbedContent")
        tabs.active = "tab-settings"
        await pilot.pause(0.4)
        body = str(app.query_one("#settings-body")._content if hasattr(app.query_one("#settings-body"), "_content") else app.query_one("#settings-body").render())
        assert "权限模式" in body and "上限" in body and "模型 profiles" in body
        await pilot.press("ctrl+f")
        await pilot.pause(0.3)
        assert h.rt.store.get_session("s1")["permissions_mode"] == "full_auto"
        await pilot.press("ctrl+f")
        await pilot.pause(0.3)
        assert h.rt.store.get_session("s1")["permissions_mode"] == "approved_scope"


async def test_tui_surfaces_run_failures_and_config_gaps(tmp_path, monkeypatch):
    """真实成员缺少模型 profile 时：启动即提示，回合失败必须在对话视图里给出原因
    （而不是静默“无返回”）。"""
    from teamagents.models import UserConfig
    from teamagents.session import open_session

    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "config"))
    project = tmp_path / "project"
    project.mkdir()
    rt = await open_session(cwd=project, session_id="missing-profile",
                            catalog=UserConfig())     # 空目录：没有 leader_main
    app = TeamAgentsApp(runtime=rt)
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause(0.4)
        prompt = app.query_one("#prompt", PromptInput)
        prompt.text = "你好"
        await pilot.press("enter")
        deadline = time.time() + 10
        text = ""
        while time.time() < deadline:
            await pilot.pause(0.3)
            text = "\n".join(str(line) for line in app.query_one("#chat-stream").lines)
            if "回合失败" in text:
                break
        assert "未配置" in text, "启动时就应提示缺失的模型 profile"
        assert "回合失败" in text and "leader_main" in text, \
            "回合失败必须在对话视图里给出原因"
    await rt.close()
    rt.store.close()


async def test_tui_owns_runtime_and_closes_it_on_exit(tmp_path, monkeypatch):
    """退出界面后运行时必须被关闭，否则 uv run 不会返回。"""
    from teamagents.session import open_session
    from teamagents.models import ModelProfile, UserConfig

    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    project = tmp_path / "project"
    project.mkdir()
    catalog = UserConfig(models={"leader_main": ModelProfile(
        provider="openai", model="test", base_url="http://127.0.0.1:9")})

    holder = {}

    async def factory():
        rt = await open_session(cwd=project, session_id="close-test", catalog=catalog)
        holder["rt"] = rt
        return rt

    app = TeamAgentsApp(runtime_factory=factory)
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause(0.3)
        assert holder["rt"].runtime_tasks_alive() if hasattr(holder["rt"], "runtime_tasks_alive") else True
    rt = holder["rt"]
    assert rt._closed, "退出后执行器应停止"
    with pytest.raises(Exception):
        rt.store.conn.execute("SELECT 1")


async def test_tui_uses_codex_like_theme(harness_factory):
    """配色参考 Codex：深底 #0d0d0d、灰 #5d5d5d 次级、白字、蓝色强调。"""
    h = await _session(harness_factory)
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause(0.4)
        assert app.theme == "codex"
        theme = app.get_theme("codex")
        assert str(theme.background).lower() == "#0d0d0d"
        assert str(theme.secondary).lower() == "#5d5d5d"
        assert str(theme.primary).lower() == "#3b82f6"
        assert str(theme.foreground).lower() == "#ffffff"
        assert str(theme.success).lower() == "#00ff00"
        # 关键部件真的用了这套配色（计算样式而非主题定义）
        def rgb(color) -> tuple:
            return (color.r, color.g, color.b)
        assert rgb(app.query_one("#status").styles.background) == (24, 24, 24)
        assert rgb(app.query_one("#side").styles.border_bottom[1]) == (93, 93, 93)
        assert rgb(app.query_one("#prompt-prefix").styles.color) == (59, 130, 246)
