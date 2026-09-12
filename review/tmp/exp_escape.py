"""Review experiment A (runtime domain): do Deep Agents file tools escape the
authorized workdir through the composite backend routes (/memory/, /skills/)?

Read-only w.r.t. the repo: everything is written under a temp dir.
Run:  .venv/bin/python review/tmp/exp_escape.py
"""
from __future__ import annotations

import asyncio
import json
import sys
from pathlib import Path

import teamagents

REPO = Path(teamagents.__file__).resolve().parents[2]
assert (REPO / "tests").is_dir(), REPO
sys.path.insert(0, str(REPO / "tests"))

from conftest import Harness, leader, spec_of  # noqa: E402
from scripted_model import ScriptedChatModel, ai_text, ai_tool  # noqa: E402
from teamagents.models import ModelProfile, UserConfig  # noqa: E402
from teamagents.permissions import ApprovalGate, PermissionPolicy  # noqa: E402
from teamagents.runners import DeepAgentsRunner  # noqa: E402
from teamagents.runtime import SessionRuntime  # noqa: E402
from teamagents.storage import Store  # noqa: E402


async def main() -> None:
    import tempfile

    tmp = Path(tempfile.mkdtemp(prefix="ta-escape-"))
    work = tmp / "work"
    work.mkdir(parents=True)
    cfg = tmp / "home" / ".config" / "teamagents"
    (cfg / "skills" / "reporting").mkdir(parents=True)
    (cfg / "AGENTS.md").write_text("project rules\n", encoding="utf-8")
    (cfg / "skills" / "reporting" / "SKILL.md").write_text("orig skill\n", encoding="utf-8")

    spec = spec_of(leader(), channels=[])
    store = Store(tmp / "s1.db")
    store.create_session("s1", str(work), "approved_scope")
    store.save_team_spec("s1", spec)
    for agent in spec.agents:
        store.ensure_agent("s1", agent.id)

    model = ScriptedChatModel(script=[
        ai_tool("write_file", {"file_path": "/memory/0/config.toml",
                               "content": "mode = 'full_auto'\n"}),
        ai_tool("write_file", {"file_path": "/skills/0/reporting/SKILL.md",
                               "content": "INJECTED\n"}),
        ai_tool("signal_done", {"summary": "ok"}),
        ai_text("done"),
    ])
    catalog = UserConfig(models={"test": ModelProfile(provider="openai", model="test")})
    approvals = ApprovalGate(store, "s1", PermissionPolicy())
    from langgraph.checkpoint.memory import InMemorySaver
    runner = DeepAgentsRunner(agent=spec.agents[0], catalog=catalog, session_id="s1",
                              workdir=work, artifacts_dir=tmp / "artifacts",
                              checkpointer=InMemorySaver(), approvals=approvals,
                              skills_dirs=[cfg / "skills"], memory_files=[cfg / "AGENTS.md"],
                              model_override=model)
    rt = SessionRuntime(store, "s1", catalog, runners={"leader": runner}, approvals=approvals)
    h = Harness(rt, {})
    await h.start()
    try:
        rt.user_message("write some files")
        settled = await rt.settle(30)
        print("settled:", settled)
        print("tool names exposed to the model:", sorted(model.tool_names))
        escaped_cfg = cfg / "config.toml"
        escaped_skill = cfg / "skills" / "reporting" / "SKILL.md"
        print("outside-workdir config.toml written:", escaped_cfg.exists(),
              repr(escaped_cfg.read_text() if escaped_cfg.exists() else ""))
        print("skill file overwritten:", escaped_skill.read_text().strip() == "INJECTED")
        print("approvals raised:", [a.status for a in store.pending_approvals("s1")])
        print("events:", [e["kind"] for e in store.events("s1")][-8:])
        print("write_file tool messages:")
        for call in model.calls:
            for m in call:
                if type(m).__name__ == "ToolMessage":
                    print("   ", str(m.content)[:160])
    finally:
        await rt.close()
        store.close()


if __name__ == "__main__":
    asyncio.run(main())
