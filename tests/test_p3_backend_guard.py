"""P3/T23: member file routes stay inside their authorized roots and the
user-owned memory/skills locations are read-only (review P0-1)."""

from __future__ import annotations

import os

from conftest import Harness, leader, spec_of
from scripted_model import ScriptedChatModel, ai_text, ai_tool
from teamagents.execution import GuardedFilesystemBackend, ReadOnlyFilesystemBackend
from teamagents.models import ModelProfile, UserConfig
from teamagents.permissions import ApprovalGate, PermissionPolicy
from teamagents.runners import DeepAgentsRunner
from teamagents.runtime import SessionRuntime
from teamagents.storage import Store


def tool_messages(model: ScriptedChatModel, name: str | None = None) -> list:
    out = [m for call in model.calls for m in call if type(m).__name__ == "ToolMessage"]
    return [m for m in out if name is None or m.name == name]


# -- backend level -----------------------------------------------------------


def test_read_only_backend_refuses_writes_and_reads_still_work(tmp_path):
    root = tmp_path / "config"
    root.mkdir()
    (root / "AGENTS.md").write_text("project rules\n", encoding="utf-8")
    backend = ReadOnlyFilesystemBackend(root, virtual_prefix="/memory/0/")

    write = backend.write("/AGENTS.md", "INJECTED\n")
    assert write.error and "read-only" in write.error
    assert (root / "AGENTS.md").read_text(encoding="utf-8") == "project rules\n"

    assert backend.edit("/AGENTS.md", "project", "no").error
    assert backend.delete("/AGENTS.md").error
    assert backend.upload_files([("/new.txt", b"x")])[0].error == "permission_denied"
    assert not (root / "new.txt").exists()

    read = backend.read("/AGENTS.md")
    assert read.error is None
    assert "project rules" in read.file_data["content"]


def test_guarded_backend_refuses_symlink_and_traversal_escapes(tmp_path):
    root = tmp_path / "root"
    root.mkdir()
    (root / "ok.txt").write_text("ok\n", encoding="utf-8")
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "secret.txt").write_text("secret\n", encoding="utf-8")
    os.symlink(outside, root / "link")

    guarded = GuardedFilesystemBackend(root)
    assert guarded.write("/../evil.txt", "x").error is not None
    assert not (tmp_path / "evil.txt").exists()

    assert guarded.write("/link/evil.txt", "x").error is not None
    assert guarded.read("/link/secret.txt").error is not None
    assert guarded.edit("/link/secret.txt", "secret", "stolen").error is not None
    assert guarded.delete("/link").error is not None
    assert not (outside / "evil.txt").exists()
    assert (outside / "secret.txt").read_text(encoding="utf-8") == "secret\n"

    # normal operations inside the root keep working
    assert guarded.write("/ok.txt", "still ok\n").error is None
    assert guarded.read("/ok.txt").file_data["content"] == "still ok\n"


# -- end to end through the member graph -------------------------------------


async def test_member_file_routes_guard_memory_skills_and_artifacts(tmp_path):
    work = tmp_path / "work"
    work.mkdir()
    artifacts = tmp_path / "artifacts"
    artifacts.mkdir()
    cfg = tmp_path / "home" / ".config" / "teamagents"
    (cfg / "skills" / "reporting").mkdir(parents=True)
    (cfg / "AGENTS.md").write_text("project rules\n", encoding="utf-8")
    skill_text = "---\nname: reporting\ndescription: write reports\n---\norig skill\n"
    (cfg / "skills" / "reporting" / "SKILL.md").write_text(skill_text, encoding="utf-8")
    outside = tmp_path / "outside"
    outside.mkdir()
    os.symlink(outside, artifacts / "escape")

    spec = spec_of(leader(), channels=[])
    store = Store(tmp_path / "s1.db")
    store.create_session("s1", str(work), "approved_scope")
    store.save_team_spec("s1", spec)
    for agent in spec.agents:
        store.ensure_agent("s1", agent.id)

    model = ScriptedChatModel(script=[
        ai_tool("write_file", {"file_path": "/memory/0/config.toml",
                               "content": "mode = 'full_auto'\n"}),
        ai_tool("write_file", {"file_path": "/skills/0/reporting/SKILL.md",
                               "content": "INJECTED\n"}),
        ai_tool("write_file", {"file_path": "/artifacts/report.txt",
                               "content": "artifact body\n"}),
        ai_tool("write_file", {"file_path": "/artifacts/escape/pwn.txt",
                               "content": "escaped\n"}),
        ai_tool("write_file", {"file_path": "/note.txt", "content": "inside\n"}),
        ai_tool("read_file", {"file_path": "/memory/0/AGENTS.md"}),
        ai_tool("signal_done", {"summary": "ok"}),
        ai_text("done"),
    ])
    catalog = UserConfig(models={"test": ModelProfile(provider="openai", model="test")})
    approvals = ApprovalGate(store, "s1", PermissionPolicy())
    from langgraph.checkpoint.memory import InMemorySaver
    runner = DeepAgentsRunner(agent=spec.agents[0], catalog=catalog, session_id="s1",
                              workdir=work, artifacts_dir=artifacts,
                              checkpointer=InMemorySaver(), approvals=approvals,
                              skills_dirs=[cfg / "skills"], memory_files=[cfg / "AGENTS.md"],
                              model_override=model)
    rt = SessionRuntime(store, "s1", catalog, runners={"leader": runner},
                        approvals=approvals)
    h = Harness(rt, {})
    await h.start()
    try:
        rt.user_message("write some files")
        assert await rt.settle(30), "session did not settle"

        # user-owned locations are untouched and the escape never lands
        assert not (cfg / "config.toml").exists()
        assert (cfg / "skills" / "reporting" / "SKILL.md").read_text() == skill_text
        assert not (outside / "pwn.txt").exists()

        # authorized writes still work
        assert (artifacts / "report.txt").read_text() == "artifact body\n"
        assert (work / "note.txt").read_text() == "inside\n"

        writes = {m.tool_call_id: m for m in tool_messages(model, "write_file")}
        assert len(writes) == 5
        errors = [m.content for m in writes.values() if m.status == "error"]
        assert len(errors) == 3, [m.content for m in writes.values()]
        assert all("read-only" in e for e in errors[:2]), errors
        assert "outside" in errors[2] and "root directory" in errors[2], errors[2]
        assert all(m.status == "success" for m in tool_messages(model, "write_file")
                   if m.content.startswith("Updated file /artifacts/report.txt")
                   or m.content.startswith("Updated file /note.txt"))

        reads = tool_messages(model, "read_file")
        assert reads and "project rules" in reads[-1].content
        assert not store.pending_approvals("s1"), "file tools must not ask for approval"
        assert rt.store.get_session("s1")["goal_state"] == "done"
        assert [r.status for r in store.runs_for_session("s1")] == ["COMPLETED"]
    finally:
        await rt.close()
        store.close()


def test_artifacts_route_is_writable_for_members(tmp_path):
    """The artifacts route must not regress into read-only (existing behavior)."""
    artifacts = tmp_path / "artifacts"
    artifacts.mkdir()
    backend = GuardedFilesystemBackend(artifacts)
    assert backend.write("/sub/out.txt", "data\n").error is None
    assert (artifacts / "sub" / "out.txt").read_text() == "data\n"
