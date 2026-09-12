"""P3 live end-to-end: a real model drives a real session through the product
path (control graph -> runtime -> Deep Agents member -> team tools)."""

from __future__ import annotations

import os

import pytest

from teamagents.models import ModelProfile, UserConfig
from teamagents.session import open_session

DEEPSEEK = ModelProfile(provider="deepseek", protocol="deepseek",
                        model="deepseek-flash", api_key_env="DEEPSEEK_API_KEY",
                        timeout=90, max_retries=1,
                        generation_options={"reasoning_effort": "high"})

pytestmark = pytest.mark.skipif(not os.environ.get("DEEPSEEK_API_KEY"),
                                reason="DEEPSEEK_API_KEY not set")
pytestmark = [pytestmark, pytest.mark.live]


async def test_live_single_leader_session_completes_goal(tmp_path, monkeypatch):
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    project = tmp_path / "project"
    project.mkdir()
    catalog = UserConfig(models={"leader_main": DEEPSEEK})
    rt = await open_session(cwd=project, session_id="live-1", catalog=catalog)
    await rt.start()
    try:
        rt.user_message(
            "请用一句话说明 1+1 等于几，然后用 signal_done 工具声明目标完成，"
            "summary 里写你的答案。")
        assert await rt.settle(120), "live session did not settle"
        session = rt.store.get_session("live-1")
        assert session["goal_state"] == "done", \
            "the Leader must complete the goal through signal_done"
        events = [e["kind"] for e in rt.store.events("live-1")]
        assert "goal_done" in events
        run = rt.store.runs_for_session("live-1")[0]
        assert run.status == "COMPLETED"
    finally:
        await rt.close()
        rt.store.close()


async def test_live_leader_delegates_to_scripted_member(tmp_path, monkeypatch):
    """T10: the Leader builds a team from natural language.

    The assertion stays at what T10 requires (valid structure applied, revision
    bumped, member usable). Whether the model then delegates a trivial errand is
    model judgement; live delegation is covered by
    `test_p5_live_codex.py` and `examples/e2e_project_fix.py`.
    """
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    project = tmp_path / "project"
    project.mkdir()
    catalog = UserConfig(models={"leader_main": DEEPSEEK})
    rt = await open_session(cwd=project, session_id="live-2", catalog=catalog)
    await rt.start()
    try:
        rt.user_message(
            "现在团队里只有你一个人。请先调用 apply_topology_patch 新增一个成员 "
            "id=worker, name=Worker, role=worker, runtime_kind=deepagents, "
            "model_profile=leader_main, tool_bindings=[files]，"
            "并加一条 leader -> worker 的 task 通道（base_revision 取当前版本）。"
            "成功后调用 assign_task 给 worker 分配任务：描述为 '回复 ok'，"
            "然后用 signal_done 结束。")
        assert await rt.settle(180), "live session did not settle"
        spec = rt.store.load_team_spec("live-2")
        assert rt.store.current_revision("live-2") >= 2, \
            "the Leader must have applied a topology patch"
        assert any(a.id == "worker" for a in spec.agents)
        tasks = rt.store.tasks_for_session("live-2")
        assert all(t.assignee in spec.agent_ids for t in tasks), \
            "any task the Leader created must target a real member"
        assert rt.store.get_session("live-2")["goal_state"] in ("done", "active")
    finally:
        await rt.close()
        rt.store.close()
