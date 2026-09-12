"""P3: MCP tool bindings (real stdio service), skills/instruction loading,
and full session bootstrap with a real SQLite checkpointer."""

from __future__ import annotations

import asyncio
import sys
from pathlib import Path

import pytest

from conftest import Harness, leader, spec_of
from scripted_model import ScriptedChatModel, ai_text, ai_tool
from teamagents.models import (
    AgentSpec,
    ModelProfile,
    RuntimeKind,
    TeamSpec,
    ToolBinding,
    UserConfig,
)
from teamagents.tools import ToolServiceUnavailable, build_bound_tools

SERVER = Path(__file__).parent / "mcp_echo_server.py"


def mcp_config(required: bool = False) -> UserConfig:
    return UserConfig(
        models={"test": ModelProfile(provider="openai", model="test")},
        tools={"echo_service": ToolBinding(
            kind="mcp", mcp_server="echo", mcp_transport="stdio",
            command=sys.executable, args=[str(SERVER)], tool_names=["echo"],
            required=required)},
    )


async def test_mcp_binding_loads_and_calls_real_stdio_server():
    catalog = mcp_config()
    tools = await build_bound_tools(catalog, ["echo_service"])
    assert len(tools) == 1
    assert tools[0].name == "echo_echo", "same-named tools get a service prefix"
    result = await tools[0].ainvoke({"text": "hi", "times": 2})
    assert "hi hi" in str(result)


async def test_missing_required_service_blocks_clearly():
    catalog = UserConfig(models={"test": ModelProfile(provider="openai", model="test")},
                         tools={"broken": ToolBinding(
                             kind="mcp", mcp_server="broken", mcp_transport="stdio",
                             command="/nonexistent/definitely-not-a-server",
                             tool_names=[], required=True)})
    with pytest.raises(ToolServiceUnavailable) as err:
        await build_bound_tools(catalog, ["broken"])
    assert "broken" in str(err.value)


async def test_optional_service_failure_only_loses_capability():
    catalog = UserConfig(models={"test": ModelProfile(provider="openai", model="test")},
                         tools={"optional_broken": ToolBinding(
                             kind="mcp", mcp_server="optional", mcp_transport="stdio",
                             command="/nonexistent/definitely-not-a-server",
                             tool_names=[], required=False)})
    tools = await build_bound_tools(catalog, ["optional_broken"])
    assert tools == []


async def test_member_can_call_bound_mcp_tool_through_the_runtime(tmp_path):
    from test_p3_deepagents_runner import build_runtime

    catalog = mcp_config()
    spec = spec_of(leader(), channels=[])
    spec.agents[0].tool_bindings = ["files", "shell", "echo_service"]
    model = ScriptedChatModel(script=[
        ai_tool("echo_echo", {"text": "ping", "times": 1}),
        ai_tool("signal_done", {"summary": "called mcp"}),
        ai_text("done"),
    ])
    rt = build_runtime(tmp_path, spec, {"leader": model})
    rt.catalog = catalog
    rt.runners["leader"].catalog = catalog
    h = Harness(rt, {})
    await h.start()
    try:
        rt.user_message("call the echo service")
        assert await rt.settle(30)
        assert "echo_echo" in model.tool_names
        outputs = [str(m.content) for call in model.calls for m in call
                   if type(m).__name__ == "ToolMessage"]
        assert any("ping" in o for o in outputs)
    finally:
        await rt.close()
        rt.store.close()


async def test_skills_and_instruction_files_reach_the_member_prompt(tmp_path):
    from test_p3_deepagents_runner import build_runtime
    from teamagents.runners import DeepAgentsRunner

    skills_dir = tmp_path / "skills"
    skill = skills_dir / "reporting"
    skill.mkdir(parents=True)
    (skill / "SKILL.md").write_text(
        "---\nname: reporting\ndescription: Write weekly status reports.\n---\n"
        "Always use the standard report template.\n", encoding="utf-8")
    agents_md = tmp_path / "AGENTS.md"
    agents_md.write_text("Project rule: answer in Chinese.", encoding="utf-8")

    spec = spec_of(leader(), channels=[])
    model = ScriptedChatModel(script=[ai_tool("signal_done", {}), ai_text("ok")])
    rt = build_runtime(tmp_path, spec, {"leader": model})
    runner = rt.runners["leader"]
    runner.skills_dirs = [skills_dir]
    runner.memory_files = [agents_md]
    h = Harness(rt, {})
    await h.start()
    try:
        rt.user_message("write the report")
        assert await rt.settle(20)
        system_prompts = " ".join(str(m.content) for call in model.calls for m in call
                                  if type(m).__name__ == "SystemMessage")
        assert "reporting" in system_prompts, "skill index must reach the system prompt"
        assert "Project rule" in system_prompts, "AGENTS.md must be loaded as memory"
    finally:
        await rt.close()
        rt.store.close()


async def test_open_session_creates_state_and_runs_a_turn(tmp_path, monkeypatch):
    import teamagents.session as session_mod
    from teamagents.session import open_session

    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "config"))
    project = tmp_path / "project"
    project.mkdir()

    model = ScriptedChatModel(script=[ai_tool("signal_done", {"summary": "ok"}),
                                      ai_text("done")])
    rt = await open_session(cwd=project, session_id="sess-test",
                            model_override_factory=lambda catalog, agent: model)
    await rt.start()
    try:
        rt.user_message("hello")
        assert await rt.settle(20)
        assert rt.store.get_session("sess-test")["goal_state"] == "done"
        base = Path(rt.store.path).parent
        assert (base / "checkpoints.sqlite").exists(), "checkpointer file must exist"
        assert (base / "artifacts").is_dir()
    finally:
        await rt.close()
        rt.store.close()
