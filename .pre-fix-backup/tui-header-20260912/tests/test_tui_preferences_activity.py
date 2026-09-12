"""Newest-first tasks, live language preferences and truthful activity feedback."""
import ast
import json
import time
from pathlib import Path

import pytest
from textual.widgets import Select, Switch, TabbedContent

from conftest import leader, member, scripts, spec_of
from teamagents.models import Task, TaskStatus, TurnRun, TurnStatus
from teamagents.tui.app import TeamAgentsApp
from teamagents.tui.i18n import ENGLISH, read_preferences, preferences_path, tr
from teamagents.tui.panels import PromptInput, TasksPanel


@pytest.fixture(autouse=True)
def ui_preferences(tmp_path, monkeypatch):
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))


async def test_tasks_newest_first_across_statuses_and_refresh_keeps_selection(harness_factory):
    h = await harness_factory(spec_of(leader()), scripts(leader=[]))
    for task_id, status, created in [("old", TaskStatus.BLOCKED, 1),
                                    ("middle", TaskStatus.SUCCEEDED, 2),
                                    ("new", TaskStatus.FAILED, 3)]:
        h.rt.store.insert_task("s1", Task(task_id=task_id, requester="leader", assignee="leader",
                                         description=task_id, status=status, created_at=created,
                                         updated_at=100-created))
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause()
        app.query_one(TabbedContent).active = "tab-tasks"
        await pilot.pause()
        panel = app.query_one(TasksPanel)
        table = app.query_one("#tasks-table")
        assert [row.key.value for row in table.ordered_rows] == ["new", "middle", "old"]
        assert table.ordered_columns[-1].label.plain == "Created"
        table.move_cursor(row=1)
        assert panel.selected_task() == "middle"
        h.rt.store.insert_task("s1", Task(task_id="newest", requester="leader", assignee="leader",
                                         description="latest", status=TaskStatus.CANCELLED,
                                         created_at=4))
        await app._refresh_widgets()
        assert [row.key.value for row in table.ordered_rows] == ["newest", "new", "middle", "old"]
        assert panel.selected_task() == "middle"


async def test_language_switch_translates_chrome_and_persists_without_touching_chat(harness_factory):
    h = await harness_factory(spec_of(leader()), scripts(leader=[]))
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(110, 40)) as pilot:
        await pilot.pause()
        tabs = app.query_one(TabbedContent)
        assert app.ui_language == "en"
        assert str(tabs.get_tab("tab-team").label) == "Team"
        assert "Session" in str(app.query_one("#status").render())
        tabs.active = "tab-settings"
        await pilot.pause()
        selector = app.query_one("#language-select", Select)
        selector.focus()
        assert selector.value == "en"
        await pilot.press("enter")
        assert selector.expanded
        await pilot.press("escape")
        assert not selector.expanded
        app._write_chat("Leader", "User content remains unchanged. 用户内容不翻译。")
        prompt = app.query_one(PromptInput)
        prompt.text = "未发送草稿"
        event_count = len(h.rt.store.events("s1"))
        await pilot.press("enter", "down", "enter")
        await pilot.pause(0.3)
        assert app.ui_language == "zh-CN"
        assert str(tabs.get_tab("tab-team").label) == "团队"
        assert "权限模式" in str(app.query_one("#settings-body").render())
        assert "会话" in str(app.query_one("#status").render())
        assert app.query_one("#team-table").ordered_columns[0].label.plain == "成员"
        assert prompt.text == "未发送草稿"
        assert len(h.rt.store.events("s1")) == event_count
        assert any("用户内容不翻译" in text for _, text in app.query_one("ChatLog")._entries)
        assert read_preferences()["language"] == "zh-CN"
        assert TeamAgentsApp().ui_language == "zh-CN"
        selector.value = "en"
        await pilot.pause(0.3)
        assert str(tabs.get_tab("tab-team").label) == "Team"
        assert read_preferences()["language"] == "en"
        assert prompt.text == "未发送草稿"


async def test_worker_activity_animates_then_waits_and_stops(harness_factory):
    h = await harness_factory(spec_of(leader(), member("b")), scripts(leader=[], b=[]))
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(100, 36)) as pilot:
        await pilot.pause()
        run = TurnRun(run_id="activity-run", session_id="s1", agent_id="b", status=TurnStatus.RUNNING,
                      config_revision=1, topology_revision=1, created_at=time.time()-65)
        h.rt.store.insert_run(run)
        await app._refresh_widgets()
        text = str(app.query_one("#activity").render())
        assert "Active agents: 1" in text and "01:" in text and "b" in text
        assert "Idle" in str(app.query_one("#composer-status").render())
        frame = app._activity_frame
        app._animate_activity()
        assert app._activity_frame != frame
        row = app.query_one("#team-table").get_row("b")
        assert "Working" in str(row[4])
        prompt = app.query_one(PromptInput)
        prompt.insert("typing during work")
        app.query_one(TabbedContent).active = "tab-settings"
        await pilot.pause()
        app.query_one("#animations-switch", Switch).value = False
        await pilot.pause(0.2)
        app._animate_activity()
        assert app._activity_frame == 0
        assert "● Working" in str(app.query_one("#activity").render())
        assert read_preferences()["animations"] is False
        h.rt.store.set_run_status(run.run_id, TurnStatus.WAITING_APPROVAL)
        await app._refresh_widgets()
        assert "Approval needed" in str(app.query_one("#activity").render())
        assert app.query_one("#activity").has_class("attention")
        h.rt.store.set_run_status(run.run_id, TurnStatus.COMPLETED)
        await app._refresh_widgets()
        assert "No turns executing" in str(app.query_one("#activity").render())
        assert not app.query_one("#activity").has_class("working")
        assert prompt.text == "typing during work"


def test_preference_validation_and_catalog_coverage():
    path = preferences_path()
    path.parent.mkdir(parents=True)
    path.write_text(json.dumps({"language": "invalid", "animations": "invalid"}))
    assert read_preferences() == {"language": "en", "animations": True}
    path.write_text("not json")
    assert read_preferences()["language"] == "en"
    root = Path(__file__).resolve().parents[1] / "src/teamagents/tui"
    for file in ("app.py", "panels.py", "approvals.py"):
        for node in ast.walk(ast.parse((root/file).read_text())):
            if (isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
                    and node.func.id == "tr" and isinstance(node.args[1], ast.Constant)):
                key = node.args[1].value
                if any("\u4e00" <= c <= "\u9fff" for c in key):
                    assert key in ENGLISH, (file, node.lineno, key)
    assert tr("en", "任务 {v0} → {v1}：{v2}", v0="1", v1="b", v2="中文任务") == "Task 1 → b: 中文任务"
