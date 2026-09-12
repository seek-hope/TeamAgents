"""B-01: the `--plain` REPL needs no runtime-side cursor and prints real replies."""

from __future__ import annotations

import inspect
import json

from teamagents import cli


def test_print_event_renders_leader_reply(capsys):
    cli._print_event({"kind": "leader_reply", "actor_id": "leader",
                      "payload_json": json.dumps({"text": "灯塔就位：" + "好" * 5000})})
    out = capsys.readouterr().out
    assert "灯塔就位" in out
    assert len(out) < 2100, "a terminal reply must be truncated to a sane length"


def test_print_event_reports_failed_runs(capsys):
    cli._print_event({"kind": "run_failed", "actor_id": "b",
                      "payload_json": json.dumps({"error": "boom"})})
    out = capsys.readouterr().out
    assert "boom" in out and "b" in out


class _Store:
    def __init__(self, events):
        self._events = events
        self.calls: list[int] = []
        self.closed = False

    def events(self, session_id, after_sequence=0, limit=1000):
        self.calls.append(after_sequence)
        return [e for e in self._events if e["sequence"] > after_sequence]

    def close(self):
        self.closed = True


class _FakeRuntime:
    """Minimal --plain runtime; deliberately has no ``ui_cursor`` attribute."""

    def __init__(self, session_id, events):
        self.session_id = session_id
        self.store = _Store(events)

    async def start(self):
        return None

    def user_message(self, text):
        return type("Receipt", (), {"result": {"goal_id": "g1"}})()

    async def settle(self, timeout=10.0):
        return True

    async def close(self):
        return None


def test_plain_loop_uses_a_local_cursor_and_prints_replies(tmp_path, monkeypatch, capsys):
    events = [
        {"sequence": 1, "kind": "leader_reply", "actor_id": "leader",
         "payload_json": json.dumps({"text": "灯塔就位"})},
        {"sequence": 2, "kind": "task_created", "actor_id": "leader",
         "payload_json": json.dumps({"task_id": "t1"})},
    ]
    runtime = _FakeRuntime("s1", events)
    assert not hasattr(runtime, "ui_cursor")

    async def fake_open_session(**kwargs):
        return runtime

    monkeypatch.setattr("teamagents.session.open_session", fake_open_session)

    lines = iter(["你好", "再问一句"])

    def fake_input(prompt=""):
        try:
            return next(lines)
        except StopIteration as e:
            raise EOFError from e

    monkeypatch.setattr("builtins.input", fake_input)

    assert cli.main(["--plain", "--cwd", str(tmp_path)]) == 0
    out = capsys.readouterr().out
    assert "灯塔就位" in out
    assert out.count("灯塔就位") == 1, "each event must be printed exactly once"
    assert runtime.store.calls == [0, 2], "the local cursor must advance per event"
    assert runtime.store.closed


def test_plain_repl_does_not_reference_ui_cursor():
    assert "ui_cursor" not in inspect.getsource(cli._repl)
