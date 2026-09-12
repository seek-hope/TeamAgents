"""Codex-style composer behavior over the production Textual widgets."""
import json

from conftest import leader, scripts, spec_of
from teamagents.control import EventDraft
from teamagents.models import EventKind, TurnRun, TurnStatus
from teamagents.tui.app import TeamAgentsApp
from teamagents.tui.panels import PromptInput


async def test_composer_multiline_history_draft_and_readline_keys(harness_factory):
    h = await harness_factory(spec_of(leader()), scripts(leader=[]))
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(80, 30)) as pilot:
        await pilot.pause()
        prompt = app.query_one(PromptInput)
        prompt.text = "第一行"
        prompt.move_cursor(prompt.document.end)
        await pilot.press("shift+enter")
        prompt.insert("第二行")
        await pilot.press("ctrl+j")
        prompt.insert("第三行")
        await pilot.pause()
        assert prompt.text == "第一行\n第二行\n第三行"
        assert prompt.region.height >= 5
        await pilot.press("enter")
        await pilot.pause(0.3)
        users = [json.loads(e["payload_json"])["text"] for e in h.rt.store.events("s1")
                 if e["kind"] == "user_message"]
        assert users == ["第一行\n第二行\n第三行"]
        prompt.text = "未发送草稿"
        prompt.move_cursor(prompt.document.end)
        await pilot.press("up")
        assert prompt.text == users[0]
        await pilot.press("down")
        assert prompt.text == "未发送草稿"
        await pilot.press("ctrl+a")
        assert prompt.cursor_location == (0, 0)
        await pilot.press("ctrl+e")
        assert prompt.cursor_location == prompt.document.end
        prompt.reset_history()
        await pilot.press("up")
        assert prompt.text == ""


async def test_stream_final_once_refresh_preserves_history_and_draft(harness_factory):
    h = await harness_factory(spec_of(leader()), scripts(leader=[]))
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(100, 36)) as pilot:
        await pilot.pause()
        run = TurnRun(run_id="streaming-run", session_id="s1", agent_id="leader",
                      config_revision=1, topology_revision=1, status=TurnStatus.RUNNING)
        h.rt.store.insert_run(run)
        h.rt.note_stream_chunk(run.run_id, "leader", "**唯一回复标记**")
        await pilot.pause(0.15)
        assert app.query_one("#chat-live").display
        h.rt.store.set_run_status(run.run_id, TurnStatus.COMPLETED)
        h.rt.control.emit([EventDraft(kind=EventKind.LEADER_REPLY,
            payload={"run_id": run.run_id, "text": "**唯一回复标记**"})])
        await pilot.pause(0.3)
        prompt = app.query_one(PromptInput)
        prompt.text = "保留草稿"
        await pilot.press("ctrl+r", "ctrl+r")
        await pilot.pause(0.3)
        text = "\n".join(str(line) for line in app.query_one("#chat-stream").lines)
        assert text.count("唯一回复标记") == 1
        assert not app.query_one("#chat-live").display
        assert app._delta_buffer == {}
        assert prompt.text == "保留草稿"


async def test_resize_reflows_chinese_reply_and_keeps_latest_content(harness_factory):
    h = await harness_factory(spec_of(leader()), scripts(leader=[]))
    app = TeamAgentsApp(runtime=h.rt)
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause()
        app._write_chat("Leader", "中文回复" * 18 + "结束标记")
        await pilot.resize_terminal(60, 26)
        await pilot.pause(0.3)
        log = app.query_one("#chat-stream")
        assert not log.show_horizontal_scrollbar
        assert log.is_vertical_scroll_end
        text = "\n".join(str(line) for line in log.lines)
        assert text.count("结束标记") == 1
