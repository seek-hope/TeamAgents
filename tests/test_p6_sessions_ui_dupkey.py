"""P6/B-07 display side: the sessions panel must render an active and an
archived record that share the same session id without raising DuplicateKey,
and every row must resolve back to the real session id for s/n/a/d actions."""

from __future__ import annotations

from teamagents.models import ModelProfile, UserConfig
from teamagents.session import open_session
from teamagents.sessions import archive_session, list_sessions
from teamagents.tui.app import TeamAgentsApp
from teamagents.tui.panels import SessionsPanel

CATALOG = UserConfig(models={"leader_main": ModelProfile(
    provider="deepseek", protocol="deepseek", model="deepseek-flash",
    api_key_env="DEEPSEEK_API_KEY")})


def _scripted_model(catalog, agent):
    from scripted_model import ScriptedChatModel, ai_text, ai_tool
    return ScriptedChatModel(script=[ai_tool("signal_done", {"summary": "ok"}),
                                     ai_text("done")])


async def test_sessions_panel_survives_active_archived_id_collision(
        tmp_path, monkeypatch):
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    project = tmp_path / "project"
    project.mkdir()

    first = await open_session(cwd=project, session_id="dup", catalog=CATALOG,
                               model_override_factory=_scripted_model)
    await first.close()
    first.store.close()
    archive_session("dup")
    # legacy state: the id is back as a fresh active session while the
    # archived record with the same id still exists
    rt = await open_session(cwd=project, session_id="dup", catalog=CATALOG,
                            model_override_factory=_scripted_model)
    try:
        infos = list_sessions(cwd=project)
        assert [(i.session_id, i.archived) for i in infos] == \
            [("dup", False), ("dup", True)]

        app = TeamAgentsApp(runtime=rt, cwd=project)
        async with app.run_test(size=(120, 40)) as pilot:
            await pilot.pause(0.3)
            await pilot.press("ctrl+t", "ctrl+t", "ctrl+t", "ctrl+t")  # 到会话面板
            await pilot.pause(0.5)
            panel = app.query_one(SessionsPanel)
            table = panel.query_one("#sessions-table")
            assert table.row_count == 2, "活动与归档同 id 两条记录都要显示"
            keys = [str(table.coordinate_to_cell_key((r, 0)).row_key.value)
                    for r in range(table.row_count)]
            assert len(set(keys)) == 2, f"行 key 必须唯一，实际 {keys}"
            for row in range(table.row_count):
                table.move_cursor(row=row)
                await pilot.pause(0.05)
                assert panel.selected_session() == "dup", \
                    "每一行都必须解析回真实会话 id"
            # 周期刷新没有吞掉 / 报告错误
            assert app._refresh_errors == (), app._refresh_errors
            chat = "\n".join(str(line) for line in app.query_one("#chat-stream").lines)
            assert "UI refresh failed" not in chat
    finally:
        await rt.close()
        rt.store.close()
