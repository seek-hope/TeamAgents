"""Config loading, TeamSpec import/export, and CLI validate/doctor surfaces."""

from __future__ import annotations

import json

import pytest

from teamagents import cli
from teamagents.config import dump_team_spec, load_team_spec
from teamagents.models import TeamSpec


SPEC = {
    "schema_version": 1,
    "leader_id": "leader",
    "agents": [
        {"id": "leader", "name": "Leader", "role": "leader", "runtime_kind": "deepagents",
         "instructions": "coordinate", "model_profile": "leader_main",
         "tool_bindings": ["files", "shell", "web"], "skills": [], "workspace_policy": "shared"},
    ],
    "channels": [],
    "observers": [],
    "shared_spaces": [{"id": "main", "readers": ["leader"], "writers": ["leader"]}],
    "limits": {"max_parallel_workers": 4, "max_members": 16, "max_turns_per_goal": 200},
}


def test_teamspec_json_yaml_roundtrip(tmp_path):
    spec = TeamSpec.model_validate(SPEC)
    for suffix in (".json", ".yaml"):
        path = tmp_path / f"team{suffix}"
        dump_team_spec(spec, path)
        again = load_team_spec(path)
        assert again.model_dump() == spec.model_dump()


def test_teamspec_rejects_unknown_fields_and_refs():
    bad = json.loads(json.dumps(SPEC))
    bad["agents"][0]["mystery_field"] = 1
    with pytest.raises(Exception):
        TeamSpec.model_validate(bad)
    bad = json.loads(json.dumps(SPEC))
    bad["channels"] = [{"source": "leader", "targets": ["ghost"], "mode": "message"}]
    with pytest.raises(Exception):
        TeamSpec.model_validate(bad)
    bad = json.loads(json.dumps(SPEC))
    bad["limits"]["max_members"] = 0
    with pytest.raises(Exception):
        TeamSpec.model_validate(bad)


def test_cli_validate(tmp_path, capsys):
    path = tmp_path / "team.yaml"
    dump_team_spec(TeamSpec.model_validate(SPEC), path)
    monkey_home = tmp_path / "home"
    monkey_home.mkdir()
    # no user config -> unknown model profile is reported, not crashed
    import os
    old = os.environ.get("XDG_CONFIG_HOME")
    os.environ["XDG_CONFIG_HOME"] = str(monkey_home)
    try:
        rc = cli.validate_spec(str(path))
        out = capsys.readouterr().out
        assert rc == 1 and "unknown model profiles" in out
    finally:
        if old is None:
            os.environ.pop("XDG_CONFIG_HOME", None)
        else:
            os.environ["XDG_CONFIG_HOME"] = old


def test_cli_doctor_smoke(capsys):
    rc = cli.doctor()
    out = capsys.readouterr().out
    assert "TeamAgents doctor" in out
    assert "bubblewrap isolation" in out
    assert rc in (0, 1)


async def test_session_lock_blocks_a_second_instance(tmp_path, monkeypatch):
    """会话记录同一时间只能被一个运行实例持有（plan §9.1 文件锁）。"""
    import pytest
    from teamagents.session import open_session
    from teamagents.sessions import SessionInUse

    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    project = tmp_path / "project"
    project.mkdir()
    first = await open_session(cwd=project, session_id="locked-1")
    try:
        with pytest.raises(SessionInUse) as err:
            await open_session(cwd=project, session_id="locked-1")
        assert "already running" in str(err.value)
    finally:
        await first.close()
        first.store.close()
    # 释放后可以再次打开
    again = await open_session(cwd=project, session_id="locked-1")
    await again.close()
    again.store.close()


def test_sessions_command_lists_records(tmp_path, monkeypatch, capsys):
    import sqlite3
    from teamagents import cli

    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    session_dir = tmp_path / "state" / "teamagents" / "sessions" / "demo-1"
    session_dir.mkdir(parents=True)
    conn = sqlite3.connect(session_dir / "team.db")
    conn.executescript(
        "CREATE TABLE sessions(session_id TEXT PRIMARY KEY, status TEXT, cwd TEXT,"
        " permissions_mode TEXT, goal_id TEXT, goal_state TEXT, created_at REAL,"
        " updated_at REAL);"
        "CREATE TABLE events(sequence INTEGER PRIMARY KEY, kind TEXT);"
        "CREATE TABLE tasks(task_id TEXT PRIMARY KEY);")
    conn.execute("INSERT INTO sessions VALUES('demo-1','ACTIVE','/tmp/x','approved_scope',"
                 "'g','done',0,1)")
    conn.execute("INSERT INTO events(kind) VALUES('user_message')")
    conn.commit()
    conn.close()
    assert cli.list_sessions(verbose=True) == 0
    out = capsys.readouterr().out
    assert "demo-1" in out and "会话记录目录" in out and "事件 1" in out


def test_xhigh_maps_to_max_for_models_without_xhigh():
    """不支持 xhigh 的模型：xhigh 自动映射为 max（用户确认的规则）。"""
    from teamagents.models import ModelProfile, UserConfig
    from teamagents.providers import build_chat_model, normalize_effort

    deepseek = ModelProfile(provider="deepseek", protocol="deepseek",
                            model="deepseek-flash", api_key_env="DEEPSEEK_API_KEY",
                            generation_options={"reasoning_effort": "xhigh"})
    options, note = normalize_effort(deepseek)
    assert options["reasoning_effort"] == "max" and note

    openai = ModelProfile(provider="openai", protocol="openai", model="gpt-6-astra",
                          generation_options={"reasoning_effort": "xhigh"})
    options, note = normalize_effort(openai)
    assert options["reasoning_effort"] == "xhigh" and note is None

    model = build_chat_model(deepseek)
    assert model.reasoning_effort == "max"


async def test_runner_falls_back_to_max_when_effort_is_rejected(tmp_path, monkeypatch):
    """供应商拒绝 xhigh 时，成员回合自动改判 max 并重建模型重试一次。"""
    from langchain_core.outputs import ChatResult
    from teamagents.models import ModelProfile, UserConfig
    from scripted_model import ScriptedChatModel, ai_text, ai_tool
    from test_p3_deepagents_runner import build_runtime
    from conftest import leader, spec_of, Harness
    import teamagents.runners as runners_mod

    class EffortPicky(ScriptedChatModel):
        reasoning_effort: str | None = None

        def _generate(self, messages, stop=None, run_manager=None, **kwargs) -> ChatResult:
            if self.reasoning_effort == "xhigh":
                raise ValueError("unsupported value for reasoning_effort: xhigh")
            return super()._generate(messages, stop, run_manager, **kwargs)

    shared_calls: list = []
    built: list = []

    def build(profile, **kwargs):
        model = EffortPicky(
            script=[ai_tool("signal_done", {}), ai_text("ok")],
            calls=shared_calls,
            reasoning_effort=(profile.generation_options or {}).get("reasoning_effort"))
        built.append(model)
        return model

    monkeypatch.setattr(runners_mod, "build_chat_model", build)

    spec = spec_of(leader(), channels=[])
    rt = build_runtime(tmp_path, spec, {"leader": ai_text("unused")})
    runner = rt.runners["leader"]
    runner.model_override = None                       # 走 profile 构建路径
    runner.catalog = UserConfig(models={"test": ModelProfile(
        provider="openai", model="test",
        generation_options={"reasoning_effort": "xhigh"})})
    rt.catalog = runner.catalog
    h = Harness(rt, {})
    await h.start()
    try:
        rt.user_message("开始")
        assert await rt.settle(15), "回退后回合应当完成"
        assert [m.reasoning_effort for m in built] == ["xhigh", "max"], \
            "第一次用 xhigh，被拒后改用 max 重建"
        assert rt.store.get_session("s1")["goal_state"] == "done"
    finally:
        await rt.close()
        rt.store.close()


def test_legacy_stored_spec_with_removed_limits_still_loads(tmp_path):
    """D-10: sessions written before the limits cleanup keep loading.

    TeamSpec *files* stay strict (typos must fail), but the spec the runtime
    itself wrote months ago must not brick a resume."""
    import time

    from teamagents.storage import Store

    store = Store(tmp_path / "legacy.db")
    try:
        store.create_session("s1", str(tmp_path), "approved_scope")
        data = TeamSpec.model_validate(SPEC).model_dump(mode="json")
        data["limits"].update({"leader_reserve": 1, "model_request_timeout_s": 120,
                               "max_auto_retries": 2})
        with store.tx():
            store.conn.execute(
                "INSERT INTO team_specs(session_id, revision, spec_json, created_at)"
                " VALUES(?,?,?,?)", ("s1", 1, json.dumps(data), time.time()))
        loaded = store.load_team_spec("s1")
        assert loaded.limits.max_model_steps_per_turn == 200
        assert "leader_reserve" not in loaded.limits.model_dump()
    finally:
        store.close()
