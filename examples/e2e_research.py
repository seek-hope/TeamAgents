"""End-to-end example 2: 联网调研并附来源（真实模型 + AnySearch）。

    DEEPSEEK_API_KEY=... ANYSEARCH_API_KEY=... python examples/e2e_research.py "问题"
"""

from __future__ import annotations

import asyncio
import os
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from teamagents.models import (AgentSpec, ChannelMode, ChannelSpec, ModelProfile,
                               RuntimeKind, TeamSpec, ToolBinding, UserConfig)
from teamagents.session import open_session


async def main() -> int:
    question = sys.argv[1] if len(sys.argv) > 1 else \
        "LangGraph 的 checkpoint 有哪些存储后端？"
    if not os.environ.get("DEEPSEEK_API_KEY") or not os.environ.get("ANYSEARCH_API_KEY"):
        print("需要 DEEPSEEK_API_KEY 与 ANYSEARCH_API_KEY")
        return 2
    root = Path(tempfile.mkdtemp(prefix="ta-research-"))
    project = root / "project"
    project.mkdir()
    os.environ.setdefault("XDG_STATE_HOME", str(root / "state"))
    catalog = UserConfig(
        models={"leader_main": ModelProfile(provider="deepseek", protocol="deepseek",
                                            model="deepseek-flash",
                                            api_key_env="DEEPSEEK_API_KEY")},
        tools={"web": ToolBinding(kind="web_search", provider="anysearch",
                                  url="https://api.anysearch.com/v1/search",
                                  api_key_env="ANYSEARCH_API_KEY"),
               "fetch": ToolBinding(kind="web_fetch", provider="anysearch")})
    spec = TeamSpec(
        leader_id="leader",
        agents=[AgentSpec(id="leader", name="Leader", role="leader",
                          runtime_kind=RuntimeKind.DEEPAGENTS,
                          instructions="调研并给出带来源 URL 的结论；用 signal_done 交付。",
                          model_profile="leader_main",
                          tool_bindings=["files", "web", "fetch"])],
        shared_spaces=[{"id": "main", "readers": ["leader"], "writers": ["leader"]}],
    )
    rt = await open_session(cwd=project, session_id="demo-research", catalog=catalog,
                            initial_spec=spec)
    await rt.start()
    try:
        rt.user_message(f"请调研：{question}\n要求：至少 2 个来源，先 web_search，"
                        "必要时 web_fetch 读正文；把结论与来源写入共享空间 main，"
                        "再 signal_done 并在 summary 里给出结论。")
        ok = await rt.settle(600)
        print("settle:", ok)
        entries = rt.store.shared_entries("demo-research", ["main"])
        for entry in entries:
            print(f"共享空间条目：{entry.content[:400]}")
        for event in rt.store.events("demo-research"):
            if event["kind"] == "goal_done":
                print("交付：", event["payload_json"][:400])
        return 0 if entries else 1
    finally:
        await rt.close()
        rt.store.close()


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
