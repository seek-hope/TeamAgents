"""P0-2/T15: the private `general-purpose` subagent stays inside the member's
permission ceiling -- no zero-approval escalation, model calls count against the
member's step budget."""

from __future__ import annotations

import asyncio

from conftest import Harness, leader, spec_of
from scripted_model import ScriptedChatModel, ai_text, ai_tool
from teamagents.models import ModelProfile, TeamAction, UserConfig
from teamagents.permissions import ApprovalGate, PermissionPolicy
from teamagents.runners import DeepAgentsRunner
from teamagents.runtime import SessionRuntime
from teamagents.storage import Store


def build_runtime(tmp_path, script):
    work = tmp_path / "work"
    work.mkdir()
    spec = spec_of(leader(), channels=[])
    store = Store(tmp_path / "s1.db")
    store.create_session("s1", str(work), "approved_scope")
    store.save_team_spec("s1", spec)
    for agent in spec.agents:
        store.ensure_agent("s1", agent.id)
    model = ScriptedChatModel(script=script)
    catalog = UserConfig(models={"test": ModelProfile(provider="openai", model="test")})
    approvals = ApprovalGate(store, "s1", PermissionPolicy())
    from langgraph.checkpoint.memory import InMemorySaver
    runner = DeepAgentsRunner(agent=spec.agents[0], catalog=catalog, session_id="s1",
                              workdir=work, artifacts_dir=tmp_path / "artifacts",
                              checkpointer=InMemorySaver(), approvals=approvals,
                              model_override=model)
    runtime = SessionRuntime(store, "s1", catalog, runners={"leader": runner},
                             approvals=approvals)
    return runtime, runner, model, store


async def test_general_purpose_subagent_cannot_escalate_without_approval(tmp_path):
    rt, runner, model, store = build_runtime(tmp_path, [
        ai_tool("task", {"subagent_type": "general-purpose",
                         "description": "check the network"}),
        ai_tool("shell", {"command": "echo sub-ok"}),                 # in scope
        ai_tool("shell", {"command": "echo NET-ESCAPED", "network": True}),
        ai_text("subagent done"),
        ai_tool("signal_done", {"summary": "ok"}),
        ai_text("done"),
    ])
    h = Harness(rt, {})
    await h.start()
    try:
        rt.user_message("check the network")
        deadline = asyncio.get_event_loop().time() + 20
        while not store.pending_approvals("s1"):
            assert asyncio.get_event_loop().time() < deadline, "no approval for `task`"
            await asyncio.sleep(0.02)
        task_approval = store.pending_approvals("s1")[0]
        assert task_approval.requested_scope["tool"] == "task"
        rt.submit(TeamAction(action_id="dec-1", session_id="s1", actor_id="user",
                             kind="approval_decision",
                             payload={"approval_id": task_approval.approval_id,
                                      "decision": "session"}))
        assert await rt.settle(30), "session did not settle"

        # the subagent's in-scope shell ran (capability kept) ...
        contents = [m.content for call in model.calls for m in call
                    if type(m).__name__ == "ToolMessage" and m.name == "shell"]
        assert any("sub-ok" in c for c in contents), contents
        # ... but the network escalation never executed, without a new approval
        assert not any("NET-ESCAPED" in c for c in contents), contents
        assert any("Blocked by team permissions" in c for c in contents), contents
        assert len(store.pending_approvals("s1")) == 0
        assert store.conn.execute("SELECT COUNT(*) AS c FROM approvals").fetchone()["c"] == 1
        # subagent model calls count against the member's own step budget:
        # after the approval resume the member itself makes only 2 model calls
        # (signal_done + final text), so >= 4 proves the subagent's 3 calls ran
        # through the shared counter
        assert 4 <= runner._middleware.model_steps <= len(model.calls), \
            (runner._middleware.model_steps, len(model.calls))
        assert store.get_session("s1")["goal_state"] == "done"
    finally:
        await rt.close()
        store.close()
