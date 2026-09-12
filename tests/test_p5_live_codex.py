"""P5/T17 live: a real Codex execution member completes a delegated task
through the production runtime (skipped when the codex CLI is absent)."""

from __future__ import annotations

import asyncio
import json
import os
import shutil
from pathlib import Path

import pytest

from conftest import Harness
from scripted_model import ScriptedChatModel, ai_text, ai_tool, last_tool_result
from teamagents.models import ModelProfile, RuntimeKind, TeamAction, UserConfig
from teamagents.session import open_session
from teamagents.models import TeamSpec, AgentSpec, ChannelMode, ChannelSpec

CODEX = shutil.which("codex")
AUTH = Path.home() / ".codex" / "auth.json"

pytestmark = pytest.mark.skipif(
    not CODEX or not AUTH.exists(), reason="codex CLI or login not available")
pytestmark = [pytestmark, pytest.mark.live]


def codex_home(tmp_path: Path) -> str:
    home = tmp_path / "codex-home"
    home.mkdir(exist_ok=True)
    shutil.copy2(AUTH, home / "auth.json")
    config = Path.home() / ".codex" / "config.toml"
    if config.exists():
        shutil.copy2(config, home / "config.toml")  # known-good local setup
    return str(home)


async def test_live_codex_member_executes_delegated_task(tmp_path, monkeypatch):
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    project = tmp_path / "project"
    project.mkdir()
    spec = TeamSpec(
        leader_id="leader",
        agents=[
            AgentSpec(id="leader", name="Leader", role="leader",
                      runtime_kind=RuntimeKind.DEEPAGENTS, instructions="coordinate",
                      model_profile="leader_main", tool_bindings=["files"]),
            AgentSpec(id="cx", name="Codex", role="worker",
                      runtime_kind=RuntimeKind.CODEX,
                      instructions="execute the delegated task",
                      model_profile="codex_worker",
                      tool_bindings=["files", "shell"]),
        ],
        channels=[ChannelSpec(source="leader", targets=["cx"], mode=ChannelMode.TASK)],
    )
    leader_model = ScriptedChatModel(script=[
        ai_tool("assign_task", {"assignee": "cx",
                                "description": "Create a file named hello.txt in the "
                                               "current working directory containing "
                                               "exactly: hi",
                                "acceptance": "hello.txt exists with content hi"}),
        lambda messages: ai_tool("wait_for_tasks", {"task_ids": [
            last_tool_result(messages)["result"]["task_id"]]}),
        ai_tool("signal_done", {"summary": "codex finished"}),
        ai_text("done"),
    ])
    catalog = UserConfig(models={
        "leader_main": ModelProfile(provider="openai", model="unused"),
        "codex_worker": ModelProfile(provider="deepseek", protocol="deepseek",
                                     model="deepseek-flash",
                                     api_key_env="DEEPSEEK_API_KEY",
                                     generation_options={"reasoning_effort": "high"})})
    rt = await open_session(cwd=project, session_id="live-codex",
                            catalog=catalog,
                            model_override_factory=lambda cat, agent: leader_model,
                            initial_spec=spec)
    cx_runner = rt.runners["cx"]
    cx_runner.codex_home = codex_home(tmp_path)
    await rt.start()
    try:
        rt.user_message("delegate the file creation to the codex member")
        # the real CLI asks for approval before writing: approve like a user would
        deadline = asyncio.get_event_loop().time() + 240
        approved = 0
        while asyncio.get_event_loop().time() < deadline:
            pending = rt.store.pending_approvals("live-codex")
            for approval in pending:
                scope = approval.requested_scope
                assert scope, "codex approval must carry its requested scope"
                decide = rt.submit(TeamAction(
                    action_id=f"approve-{approval.approval_id}", session_id="live-codex",
                    actor_id="user", kind="approval_decision",
                    payload={"approval_id": approval.approval_id, "decision": "once"}))
                assert decide.ok, decide.error
                approved += 1
            if not rt.store.runs_for_session("live-codex", ["QUEUED", "RUNNING"]) \
                    and not rt._inflight and not pending:
                break
            await asyncio.sleep(0.5)
        assert approved >= 1, "the live Codex turn should have asked for approval"
        assert await rt.settle(120), "live codex session did not settle"
        tasks = rt.store.tasks_for_session("live-codex")
        assert tasks and tasks[0].status == "SUCCEEDED", \
            f"task state: {tasks[0].status if tasks else 'missing'}"
        assert (project / "hello.txt").exists(), "codex must have created the file"
        assert (project / "hello.txt").read_text().strip() == "hi"
        progress = [e["payload_json"] for e in rt.store.events("live-codex")
                    if e["kind"] == "run_progress"]
        assert progress, "codex progress must be mapped into team events"
        assert rt.store.get_codex_thread("live-codex", "cx")
    finally:
        await rt.close()
        rt.store.close()


_ = (asyncio, json, os)
