"""TUI UX: cursor preservation across periodic refresh, approval alerts,
persistent composer history, log-filter exit."""

from __future__ import annotations

import pytest

from conftest import Harness, leader, member, msg_channel, scripts, spec_of, task_channel
from teamagents.control import EventDraft
from teamagents.models import (ApprovalRequest, ApprovalStatus, EventKind)
from teamagents.tui.app import TeamAgentsApp
from teamagents.tui.approvals import ApprovalsPanel
from teamagents.tui.i18n import read_history
from teamagents.tui.panels import PromptInput


async def _session(harness_factory):
    spec = spec_of(leader(), member("b"),
                   channels=[task_channel("leader", ["b"]), msg_channel("b", ["leader"])])
    members = scripts(leader=[("call", "signal_done", {"summary": "hi"}), ("end",)],
                      b=[("end",)])
    return await harness_factory(spec, members)


def _approval(approval_id: str, index: int = 0) -> ApprovalRequest:
    return ApprovalRequest(
        approval_id=approval_id, session_id="s1", agent_id="b", run_id="run-x",
        tool_call_id=f"call-{index}", operation_hash=f"hash-{approval_id}",
        requested_scope={"tool": "shell", "args": {"command": f"cmd-{index}"}},
        policy_revision=1)


async def test_approvals_cursor_survives_refresh_and_tracks_row(harness_factory):
    h = await _session(harness_factory)
    for i in range(3):
        with h.rt.store.tx():
            h.rt.store.insert_approval(_approval(f"appr-{i}", i))
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause()
        await pilot.press("ctrl+g")
        await pilot.pause(0.6)
        panel = app.query_one(ApprovalsPanel)
        table = panel.query_one("#approvals-table")
        table.focus()
        await pilot.pause(0.1)
        await pilot.press("down")
        await pilot.pause(0.1)
        assert panel.selected_approval() == "appr-1"
        await app._refresh_widgets()           # the 1s periodic rebuild
        await pilot.pause(0.1)
        assert panel.selected_approval() == "appr-1", "周期刷新不能抢走光标"
        # consume the row above the cursor: the cursor must stay on appr-1
        assert ApprovalsPanel.decide(h.rt, "appr-0", "once")
        await app._refresh_widgets()
        await pilot.pause(0.1)
        assert table.row_count == 2
        assert panel.selected_approval() == "appr-1", "行被消费后光标要跟随原选中行"
        # consume the selected row itself: the successor slides into its slot
        assert ApprovalsPanel.decide(h.rt, "appr-1", "once")
        await app._refresh_widgets()
        await pilot.pause(0.1)
        assert panel.selected_approval() == "appr-2", "选中行被处理后落到下一条"


async def test_approval_event_alerts_only_while_pending(harness_factory):
    h = await _session(harness_factory)
    with h.rt.store.tx():
        h.rt.store.insert_approval(_approval("appr-toast"))
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause()
        h.rt.control.emit([EventDraft(kind=EventKind.APPROVAL_REQUESTED,
            payload={"approval_id": "appr-toast", "agent_id": "b",
                     "scope": {"tool": "shell", "args": {"command": "ls"}}})])
        await pilot.pause(0.3)
        assert len(app._notifications) >= 1, "待批准必须主动提醒"
        # a decided approval replayed from history must not re-alert
        assert ApprovalsPanel.decide(h.rt, "appr-toast", "deny")
        before = len(app._notifications)
        h.rt.control.emit([EventDraft(kind=EventKind.APPROVAL_REQUESTED,
            payload={"approval_id": "appr-toast", "agent_id": "b",
                     "scope": {"tool": "shell", "args": {"command": "ls"}}})])
        await pilot.pause(0.3)
        assert len(app._notifications) == before, "已决定的批准不得重复提醒"


async def test_composer_history_persists(harness_factory):
    h = await _session(harness_factory)
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(100, 36)) as pilot:
        await pilot.pause()
        prompt = app.query_one(PromptInput)
        prompt.text = "持久化这条输入"
        await pilot.press("enter")
        await pilot.pause(0.3)
        assert read_history() == ["持久化这条输入"], "提交后要写入历史文件"
    # a fresh widget (new mount) reloads the same history
    fresh = PromptInput()
    fresh.on_mount()
    assert fresh.prompt_history == ["持久化这条输入"]
    fresh.record_submission("第二条")
    assert read_history() == ["持久化这条输入", "第二条"]
    fresh.reset_history()
    assert read_history() == []


async def test_team_table_enter_clears_log_filter(harness_factory):
    h = await _session(harness_factory)
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause()
        table = app.query_one("#team-table")
        # unfocused rebuilds must not hijack the filter
        await app._refresh_widgets()
        await pilot.pause(0.1)
        assert app._selected_member is None
        table.focus()
        await pilot.pause(0.1)
        await pilot.press("down")
        await pilot.pause(0.1)
        assert app._selected_member == "b", "高亮成员即筛选其日志"
        await pilot.press("enter")
        await pilot.pause(0.1)
        assert app._selected_member is None, "Enter 取消成员筛选"


async def test_task_detail_goes_to_toast_not_chat(harness_factory):
    spec = spec_of(leader(), member("b"),
                   channels=[task_channel("leader", ["b"]), msg_channel("b", ["leader"])])
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "toast-detail-marker"}),
                ("call", "signal_done", {}), ("end",)],
        b=[("call", "complete_task", {"task_id": "$inbox0.payload.task_id"}), ("end",)])
    h = await harness_factory(spec, members)
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause()
        await h.user("开始")
        await h.settle(15)
        tabs = app.query_one("TabbedContent")
        tabs.active = "tab-tasks"
        await pilot.pause(0.4)
        table = app.query_one("#tasks-table")
        assert table.row_count == 1
        table.focus()
        await pilot.pause(0.1)
        before = "\n".join(str(line) for line in app.query_one("#chat-stream").lines)
        await pilot.press("enter")
        await pilot.pause(0.3)
        assert any("toast-detail-marker" in n.message for n in app._notifications), \
            "任务详情应弹 toast"
        after = "\n".join(str(line) for line in app.query_one("#chat-stream").lines)
        assert after == before, "查看任务详情不得写入聊天记录"
