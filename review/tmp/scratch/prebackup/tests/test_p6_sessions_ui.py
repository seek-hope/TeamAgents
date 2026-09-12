"""P6: switching between sessions in one directory, plus archive/delete."""

from __future__ import annotations

import asyncio
import os
import time

import pytest

from conftest import Harness, leader, member, scripts, spec_of, task_channel
from teamagents.models import ModelProfile, UserConfig
from teamagents.sessions import (SessionDeleteBlocked, SessionInUse, archive_session,
                                 delete_session, is_session_locked, list_sessions,
                                 new_session_id)
from teamagents.session import open_session
from teamagents.tui.app import TeamAgentsApp
from teamagents.tui.approvals import ApprovalsPanel
from teamagents.tui.panels import PromptInput, SessionsPanel

CATALOG = UserConfig(models={"leader_main": ModelProfile(
    provider="deepseek", protocol="deepseek", model="deepseek-flash",
    api_key_env="DEEPSEEK_API_KEY")})


async def _open(tmp_path, monkeypatch, session_id: str):
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    project = tmp_path / "project"
    project.mkdir(exist_ok=True)
    return await open_session(cwd=project, session_id=session_id, catalog=CATALOG,
                              model_override_factory=_scripted_model)


def _scripted_model(catalog, agent):
    from scripted_model import ScriptedChatModel, ai_text, ai_tool
    return ScriptedChatModel(script=[ai_tool("signal_done", {"summary": "ok"}),
                                     ai_text("done")])


def test_inventory_new_id_archive_and_delete(tmp_path, monkeypatch):
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    project = tmp_path / "project"
    project.mkdir()

    async def scenario():
        first = await open_session(cwd=project, session_id="proj_a", catalog=CATALOG,
                                   model_override_factory=_scripted_model)
        await first.close()
        first.store.close()
        # 默认会话 id 尚不存在 -> 新建就是它；已存在 -> 递增后缀
        second_id = new_session_id(project)
        assert second_id != "proj_a"
        second = await open_session(cwd=project, session_id=second_id, catalog=CATALOG,
                                    model_override_factory=_scripted_model)
        await second.close()
        second.store.close()
        third_id = new_session_id(project)
        assert third_id == f"{second_id}_2", "第三个会话要拿到递增 id"
        third = await open_session(cwd=project, session_id=third_id, catalog=CATALOG,
                                   model_override_factory=_scripted_model)
        await third.close()
        third.store.close()
        return second_id

    second_id = asyncio.run(scenario())
    rows = list_sessions(cwd=project)
    assert {r.session_id for r in rows} == {"proj_a", second_id, f"{second_id}_2"}
    assert all(r.cwd.endswith("project") for r in rows)
    assert list_sessions(cwd=tmp_path / "elsewhere") == []

    target = archive_session("proj_a")
    assert target.exists() and not (tmp_path / "state" / "teamagents" / "sessions"
                                    / "proj_a").exists()
    rows = list_sessions(cwd=project)
    assert {r.session_id for r in rows if not r.archived} == {second_id, f"{second_id}_2"}
    assert any(r.archived and r.session_id == "proj_a" for r in rows)

    delete_session(second_id)
    assert not (tmp_path / "state" / "teamagents" / "sessions" / second_id).exists()
    with pytest.raises(FileNotFoundError):
        delete_session("ghost")


def test_delete_refuses_while_running_and_with_unmerged_work(tmp_path, monkeypatch):
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    project = tmp_path / "project"
    project.mkdir()

    async def live_session():
        rt = await open_session(cwd=project, session_id="proj_live", catalog=CATALOG,
                                model_override_factory=_scripted_model)
        return rt

    rt = asyncio.run(live_session())
    try:
        assert is_session_locked("proj_live")
        with pytest.raises(SessionInUse):
            archive_session("proj_live")
        with pytest.raises(SessionInUse):
            delete_session("proj_live")
    finally:
        asyncio.run(rt.close())
        rt.store.close()
    assert not is_session_locked("proj_live")
    archive_session("proj_live")
    assert (tmp_path / "state" / "teamagents" / "sessions" / "archived"
            / "proj_live").is_dir()


async def test_tui_switches_between_sessions_in_one_directory(tmp_path, monkeypatch):
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    project = tmp_path / "project"
    project.mkdir()
    rt = await open_session(cwd=project, session_id="one", catalog=CATALOG,
                            model_override_factory=_scripted_model)
    other = await open_session(cwd=project, session_id="two", catalog=CATALOG,
                               model_override_factory=_scripted_model)
    await other.close()
    other.store.close()
    try:
        app = TeamAgentsApp(runtime=rt, cwd=project, open_kwargs={"catalog": CATALOG,
                                                                  "model_override_factory": _scripted_model})
        async with app.run_test(size=(120, 40)) as pilot:
            await pilot.pause(0.3)
            await pilot.press("ctrl+t", "ctrl+t", "ctrl+t", "ctrl+t")   # 到会话面板
            await pilot.pause(0.4)
            panel = app.query_one(SessionsPanel)
            table = panel.query_one("#sessions-table")
            assert table.row_count == 2, "会话面板列出本目录的两个会话"
            table.focus()
            # 选中另一条会话并切换
            rows = [str(table.coordinate_to_cell_key((r, 0)).row_key.value)
                    for r in range(table.row_count)]
            target = "two" if "two" in rows else "one"
            table.move_cursor(row=rows.index(target))
            await pilot.pause(0.1)
            await pilot.press("s")
            deadline = time.time() + 5
            while app.rt is None or app.rt.session_id != target:
                assert time.time() < deadline, "切换未完成"
                await pilot.pause(0.1)
            assert app.rt.session_id == target
            assert is_session_locked(target)
            assert not is_session_locked("one")
            text = "\n".join(str(line) for line in
                             app.query_one("#chat-stream").lines)
            assert f"已切换到会话 {target}" in text
    finally:
        if app.rt is not None:
            await app.rt.close()
            app.rt.store.close()


async def test_tui_archive_and_delete_other_session(tmp_path, monkeypatch):
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    project = tmp_path / "project"
    project.mkdir()
    current = await open_session(cwd=project, session_id="cur", catalog=CATALOG,
                                 model_override_factory=_scripted_model)
    other = await open_session(cwd=project, session_id="other", catalog=CATALOG,
                               model_override_factory=_scripted_model)
    await other.close()
    other.store.close()
    try:
        app = TeamAgentsApp(runtime=current, cwd=project,
                            open_kwargs={"catalog": CATALOG,
                                         "model_override_factory": _scripted_model})
        async with app.run_test(size=(120, 40)) as pilot:
            await pilot.pause(0.3)
            await pilot.press("ctrl+t", "ctrl+t", "ctrl+t", "ctrl+t")
            await pilot.pause(0.4)
            panel = app.query_one(SessionsPanel)
            table = panel.query_one("#sessions-table")
            table.focus()
            rows = [str(table.coordinate_to_cell_key((r, 0)).row_key.value)
                    for r in range(table.row_count)]
            table.move_cursor(row=rows.index("other"))
            await pilot.pause(0.1)
            await pilot.press("a")                    # 归档 other（非当前会话）
            deadline = time.time() + 5
            archived = tmp_path / "state" / "teamagents" / "sessions" / "archived" / "other"
            while not archived.is_dir():
                assert time.time() < deadline, "归档未完成"
                await pilot.pause(0.1)
            assert app.rt.session_id == "cur", "归档其他会话不应切换当前会话"
    finally:
        if app.rt is not None:
            await app.rt.close()
            app.rt.store.close()


async def test_tui_deleting_current_session_exits(tmp_path, monkeypatch):
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    project = tmp_path / "project"
    project.mkdir()
    rt = await open_session(cwd=project, session_id="doomed", catalog=CATALOG,
                            model_override_factory=_scripted_model)
    app = TeamAgentsApp(runtime=rt, cwd=project,
                        open_kwargs={"catalog": CATALOG,
                                     "model_override_factory": _scripted_model})
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause(0.3)
        await pilot.press("ctrl+t", "ctrl+t", "ctrl+t", "ctrl+t")
        await pilot.pause(0.4)
        panel = app.query_one(SessionsPanel)
        table = panel.query_one("#sessions-table")
        table.focus()
        rows = [str(table.coordinate_to_cell_key((r, 0)).row_key.value)
                for r in range(table.row_count)]
        table.move_cursor(row=rows.index("doomed"))
        await pilot.pause(0.1)
        await pilot.press("d")        # 第一次：要求确认
        await pilot.pause(0.2)
        assert panel.pending_delete == "doomed"
        assert not rt._closed
        await pilot.press("d")        # 第二次：真正删除并退出
        deadline = time.time() + 5
        target = tmp_path / "state" / "teamagents" / "sessions" / "doomed"
        while target.exists():
            assert time.time() < deadline, "删除未完成"
            await pilot.pause(0.1)
        while app.rt is not None:
            assert time.time() < deadline, "删除当前会话后应退出"
            await pilot.pause(0.1)
    assert not target.exists()
    assert app._runtime_closed
    _ = (Harness, ApprovalsPanel, leader, member, scripts, spec_of, task_channel,
         os, PromptInput)
