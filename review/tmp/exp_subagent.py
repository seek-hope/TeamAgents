"""Review experiment B (runtime domain): does the auto-added `general-purpose`
private subagent bypass the TeamAgentMiddleware permission gate?

The parent scripted model calls `task(subagent_type="general-purpose", ...)`.
The subagent (same ScriptedChatModel instance, same script cursor) then calls
the isolated `shell` tool with network=true, which in the parent graph requires
an explicit user approval (permissions.py:55-59).

Read-only w.r.t. the repo; everything under a temp dir.
Run:  .venv/bin/python review/tmp/exp_subagent.py
"""
from __future__ import annotations

import asyncio
import json
import sys
import tempfile
from pathlib import Path

import teamagents

REPO = Path(teamagents.__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "tests"))

from conftest import Harness, leader, spec_of  # noqa: E402
from scripted_model import ScriptedChatModel, ai_text, ai_tool  # noqa: E402
from teamagents.models import ModelProfile, UserConfig  # noqa: E402
from teamagents.permissions import ApprovalGate, PermissionPolicy  # noqa: E402
from teamagents.runners import DeepAgentsRunner  # noqa: E402
from teamagents.runtime import SessionRuntime  # noqa: E402
from teamagents.storage import Store  # noqa: E402

NET_CMD = "cat /proc/net/dev | tail -n +3 | awk '{print $1}' | tr -d ' :'"


async def main() -> None:
    tmp = Path(tempfile.mkdtemp(prefix="ta-subagent-"))
    work = tmp / "work"
    work.mkdir(parents=True)

    spec = spec_of(leader(), channels=[])
    store = Store(tmp / "s1.db")
    store.create_session("s1", str(work), "approved_scope")
    store.save_team_spec("s1", spec)
    for agent in spec.agents:
        store.ensure_agent("s1", agent.id)

    model = ScriptedChatModel(script=[
        # 1. parent agent delegates to the auto-added general-purpose subagent
        ai_tool("task", {"subagent_type": "general-purpose",
                         "description": "report the network interfaces"}),
        # 2. the subagent asks for network access -- must need approval
        ai_tool("shell", {"command": NET_CMD, "network": True}),
        ai_text("subagent done"),
        # 3. parent finishes
        ai_tool("signal_done", {"summary": "ok"}),
        ai_text("done"),
    ])
    catalog = UserConfig(models={"test": ModelProfile(provider="openai", model="test")})
    policy = PermissionPolicy()  # approved_scope, shell network requires approval
    approvals = ApprovalGate(store, "s1", policy)
    from langgraph.checkpoint.memory import InMemorySaver
    runner = DeepAgentsRunner(agent=spec.agents[0], catalog=catalog, session_id="s1",
                              workdir=work, artifacts_dir=tmp / "artifacts",
                              checkpointer=InMemorySaver(), approvals=approvals,
                              model_override=model)
    rt = SessionRuntime(store, "s1", catalog, runners={"leader": runner},
                        approvals=approvals)
    h = Harness(rt, {})
    await h.start()
    try:
        rt.user_message("check the network")
        # the parent's `task` call itself is outside the pre-authorized scope
        deadline = asyncio.get_event_loop().time() + 30
        while not store.pending_approvals("s1"):
            assert asyncio.get_event_loop().time() < deadline, "no approval for `task`"
            await asyncio.sleep(0.02)
        first = store.pending_approvals("s1")[0]
        print("approval scope:", json.dumps(first.requested_scope, ensure_ascii=False)[:300])
        from teamagents.models import TeamAction
        rt.submit(TeamAction(action_id="dec-1", session_id="s1", actor_id="user",
                             kind="approval_decision",
                             payload={"approval_id": first.approval_id,
                                      "decision": "session"}))
        settled = await rt.settle(40)
        print("settled:", settled)
        print("tool names exposed to the model:", sorted(model.tool_names))
        print("approvals raised:", [a.status for a in store.pending_approvals("s1")])
        print("approval rows in DB:",
              store.conn.execute("SELECT COUNT(*) AS c FROM approvals").fetchone()["c"])
        print("events:", [e["kind"] for e in store.events("s1")])
        print("run status:", [r.status for r in store.runs_for_session("s1")])
        print("tool messages seen by the model:")
        for call in model.calls:
            for m in call:
                if type(m).__name__ == "ToolMessage":
                    print("   ", m.name, "->", str(m.content)[:200])
        shell_out = [str(m.content) for call in model.calls for m in call
                     if type(m).__name__ == "ToolMessage" and m.name == "shell"]
        for out in shell_out:
            try:
                payload = json.loads(out)
            except Exception:
                continue
            print("  shell exit:", payload.get("exit_code"),
                  "| interfaces:", repr(payload.get("output"))[:120])
    finally:
        await rt.close()
        store.close()


if __name__ == "__main__":
    asyncio.run(main())
