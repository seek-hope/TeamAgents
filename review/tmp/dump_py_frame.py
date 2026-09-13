"""Dump the Python (Textual) TUI frame as plain text for parity diffing with the
Rust TUI (tui/tests/render_tests.rs::frame_dump reads the same scenario JSON).

Usage: .venv/bin/python review/tmp/dump_py_frame.py OUT.txt [WIDTH] [HEIGHT] [PANEL] [LANG]
PANEL: 0=Team 1=Tasks 2=Shared 3=Approvals 4=Sessions 5=Log 6=Settings
"""
import asyncio, json, sys
from pathlib import Path
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tests"))
sys.path.insert(0, str(ROOT / "src"))

from teamagents.agents import FakeMember
from teamagents.models import (ApprovalRequest, ApprovalStatus, SharedEntry, Task,
                               TeamEvent, TeamSpec, UserConfig, ModelProfile, new_id)
from teamagents.runtime import fake_session
from teamagents.tui.app import TeamAgentsApp

SCENARIO = json.loads((Path(__file__).with_name("parity_scenario.json")).read_text())


def seed(store, session_id):
    for task in SCENARIO["tasks"]:
        store.insert_task(session_id, Task.model_validate(task))
    for entry in SCENARIO["shared_entries"]:
        store.add_shared_entry(SharedEntry.model_validate(entry), session_id)
    for approval in SCENARIO["pending_approvals"]:
        store.insert_approval(ApprovalRequest.model_validate(approval))
    for event in SCENARIO["events"]:
        store.append_event(TeamEvent(
            event_id=new_id("evt"), session_id=session_id, sequence=event["sequence"],
            actor_id=event["actor_id"], kind=event["kind"], payload=event["payload"],
            audience=["leader"], topology_revision=1))


def screen_text(screen) -> str:
    out = []
    for strip in screen._compositor.render_strips():
        out.append("".join(seg.text for seg in getattr(strip, "_segments", [])).rstrip())
    return "\n".join(out)


async def main(out, size, panel, lang):
    with TemporaryDirectory() as tmp:
        spec = TeamSpec.model_validate(SCENARIO["spec"])
        members = {name: FakeMember(name, []) for name in ("leader", "researcher", "coder")}
        rt = fake_session(tmp, spec, members,
                          catalog=UserConfig(models={"test": ModelProfile(provider="openai", model="test")}))
        seed(rt.store, rt.session_id)
        app = TeamAgentsApp(runtime=rt)
        async with app.run_test(size=size) as pilot:
            await pilot.pause(0.3)
            app.ui_language = lang
            app._apply_language()
            app.query_one("#prompt").text = SCENARIO["composer_text"]
            app.query_one("TabbedContent").active = [
                "tab-team", "tab-tasks", "tab-shared", "tab-approvals",
                "tab-sessions", "tab-log", "tab-settings"][panel]
            await pilot.pause(0.6)  # let the event loop render the seeded events
            Path(out).write_text(screen_text(app.screen), encoding="utf-8")
        await rt.close()
        rt.store.close()


if __name__ == "__main__":
    out = sys.argv[1]
    size = (int(sys.argv[2]), int(sys.argv[3])) if len(sys.argv) > 3 else (110, 32)
    panel = int(sys.argv[4]) if len(sys.argv) > 4 else 0
    lang = sys.argv[5] if len(sys.argv) > 5 else "en"
    asyncio.run(main(out, size, panel, lang))
