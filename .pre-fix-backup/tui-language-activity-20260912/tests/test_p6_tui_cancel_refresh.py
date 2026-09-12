"""P6 follow-up (A-03 / B-02 / B-06): cancel a task from the task panel and
verify the periodic refresh actually repaints status/panels/log."""

from __future__ import annotations

from conftest import leader, member, msg_channel, scripts, spec_of, task_channel
from teamagents.models import (
    ActionKind,
    EventKind,
    Task,
    TaskStatus,
    TeamAction,
    TeamEvent,
    TurnRun,
)
from teamagents.tui.app import TeamAgentsApp
from teamagents.tui.panels import StatusBar, TasksPanel


async def _session(harness_factory, *, b_script=None):
    spec = spec_of(leader(), member("b"),
                   channels=[task_channel("leader", ["b"]), msg_channel("b", ["leader"])])
    return await harness_factory(spec, scripts(leader=[("end",)],
                                               b=b_script or [("end",)]))


def _task(task_id: str, status: TaskStatus, assignee: str = "b") -> Task:
    return Task(task_id=task_id, requester="leader", assignee=assignee,
                description=f"work {task_id}", status=status)


def _seed(rt, task: Task, run: TurnRun | None = None) -> None:
    with rt.store.tx():
        rt.store.insert_task(rt.session_id, task)
        if run is not None:
            rt.store.insert_run(run)


async def _open_tasks(pilot, app) -> "object":
    await pilot.pause(0.4)
    app.query_one("TabbedContent").active = "tab-tasks"
    await pilot.pause(0.4)
    table = app.query_one(TasksPanel).query_one("#tasks-table")
    table.move_cursor(row=0)
    table.focus()
    await pilot.pause(0.1)
    return table


def _chat_text(app) -> str:
    return "\n".join(str(line) for line in app.query_one("#chat-stream").lines)


async def test_tui_task_panel_cancels_blocked_task(harness_factory):
    """A-03：BLOCKED 任务（被中断回合留下的唯一出口）可在任务面板按 c 取消。"""
    h = await _session(harness_factory)
    _seed(h.rt, _task("task-blocked-1", TaskStatus.BLOCKED))
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(120, 40)) as pilot:
        table = await _open_tasks(pilot, app)
        assert table.row_count == 1
        await pilot.press("c")
        await pilot.pause(0.3)
        assert h.rt.store.get_task("task-blocked-1").status == TaskStatus.CANCELLED
        assert "已取消" in _chat_text(app), "取消结果必须有界面反馈"


async def test_tui_task_panel_requests_cancel_of_running_task(harness_factory):
    """RUNNING 且有活动回合：只发出取消请求（回合收到 CANCEL_REQUESTED），任务不直接终结。"""
    h = await _session(harness_factory, b_script=[("sleep", 5), ("end",)])
    _seed(h.rt, _task("task-running-1", TaskStatus.RUNNING),
          TurnRun(run_id="run-running-1", session_id="s1", task_id="task-running-1",
                  agent_id="b", config_revision=1, topology_revision=1,
                  status="RUNNING"))
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(120, 40)) as pilot:
        await _open_tasks(pilot, app)
        await pilot.press("c")
        await pilot.pause(0.3)
        assert h.rt.store.run_cancel_requested("run-running-1"), \
            "取消请求必须落到活动回合上"
        assert "已请求取消" in _chat_text(app)


async def test_tui_task_panel_reports_terminal_task_as_noop(harness_factory):
    """终态任务再按 c 只提示无需取消，不改动状态。"""
    h = await _session(harness_factory)
    _seed(h.rt, _task("task-done-1", TaskStatus.SUCCEEDED))
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(120, 40)) as pilot:
        await _open_tasks(pilot, app)
        await pilot.press("c")
        await pilot.pause(0.3)
        assert h.rt.store.get_task("task-done-1").status == TaskStatus.SUCCEEDED
        assert "无需取消" in _chat_text(app)


async def test_tui_periodic_refresh_updates_status_and_panels(harness_factory):
    """B-02：状态栏查询不再因类型不匹配整段失效，周期刷新会更新状态栏与面板。"""
    h = await _session(harness_factory)
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause(0.4)
        status = app.query_one("#status", StatusBar)   # must not raise WrongType
        first = str(status.render())
        assert "会话" in first and "权限" in first, f"状态栏内容为空：{first!r}"
        assert app._refresh_errors == (), app._refresh_errors

        tasks = app.query_one(TasksPanel).query_one("#tasks-table")
        before = tasks.row_count
        _seed(h.rt, _task("task-refresh-1", TaskStatus.PENDING))
        await pilot.pause(1.3)                         # one periodic tick
        assert tasks.row_count == before + 1, "周期刷新必须更新面板内容"

        assert h.rt.submit(TeamAction(
            action_id="ui-test-mode", session_id="s1", actor_id="user",
            kind=ActionKind.SET_PERMISSION_MODE,
            payload={"mode": "full_auto"})).ok
        await pilot.pause(1.3)
        assert "full_auto" in str(status.render()), "状态变化要反映到状态栏"
        assert app._refresh_errors == (), app._refresh_errors


async def test_tui_log_panel_refreshes_without_eating_chat_cursor(harness_factory):
    """B-06：日志面板纳入周期刷新；其游标独立，不能吞掉聊天事件（B-05 首条）。"""
    h = await _session(harness_factory)
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause(0.4)
        app.query_one("TabbedContent").active = "tab-log"
        await pilot.pause(0.4)
        assert len(app.query_one("#log-stream").lines) == 0

        with h.rt.store.tx():
            h.rt.store.append_event(TeamEvent(
                event_id="e-log-1", session_id="s1", actor_id="leader",
                kind=EventKind.TASK_CREATED,
                payload={"task_id": "task-log", "assignee": "leader",
                         "description": "log-visible-marker"},
                audience=["leader"]))
        await pilot.pause(1.3)                         # one periodic tick
        lines = "\n".join(str(line) for line in app.query_one("#log-stream").lines)
        assert "log-visible-marker" in lines, "日志面板必须随周期刷新出现新事件"
        assert "log-visible-marker" in _chat_text(app), \
            "日志面板刷新不得推进聊天游标"
