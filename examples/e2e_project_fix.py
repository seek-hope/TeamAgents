"""End-to-end example 1: 项目修改并测试（真实模型）。

    DEEPSEEK_API_KEY=... python examples/e2e_project_fix.py /tmp/demo-project

Leader 组建一个编码成员，修好示例项目里失败的测试，再由 Leader 合并与验收。
"""

from __future__ import annotations

import asyncio
import os
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from teamagents.models import (AgentSpec, ChannelMode, ChannelSpec, ModelProfile,
                               RuntimeKind, TeamSpec, UserConfig)
from teamagents.session import open_session


def seed_project(root: Path) -> Path:
    project = root / "demo-project"
    project.mkdir(parents=True, exist_ok=True)
    (project / "calc.py").write_text("def add(a, b):\n    return a - b\n")
    (project / "test_calc.py").write_text(
        "from calc import add\n\n\ndef test_add():\n    assert add(2, 3) == 5\n")
    subprocess.run(["git", "init", "-q"], cwd=project, check=True)
    subprocess.run(["git", "config", "user.email", "demo@example.com"], cwd=project,
                   check=True)
    subprocess.run(["git", "config", "user.name", "Demo"], cwd=project, check=True)
    subprocess.run(["git", "add", "-A"], cwd=project, check=True)
    subprocess.run(["git", "commit", "-qm", "seed broken add"], cwd=project, check=True)
    return project


async def main() -> int:
    if not os.environ.get("DEEPSEEK_API_KEY"):
        print("需要 DEEPSEEK_API_KEY")
        return 2
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(tempfile.mkdtemp(prefix="ta-demo-"))
    project = seed_project(root)
    os.environ.setdefault("XDG_STATE_HOME", str(root / "state"))
    catalog = UserConfig(models={
        "leader_main": ModelProfile(provider="deepseek", protocol="deepseek",
                                    model="deepseek-flash",
                                    api_key_env="DEEPSEEK_API_KEY"),
        "coding": ModelProfile(provider="deepseek", protocol="deepseek",
                               model="deepseek-flash", api_key_env="DEEPSEEK_API_KEY")})
    spec = TeamSpec(
        leader_id="leader",
        agents=[
            AgentSpec(id="leader", name="Leader", role="leader",
                      runtime_kind=RuntimeKind.DEEPAGENTS,
                      instructions="协调团队：验证成果后合并成员分支，并用 signal_done 交付。",
                      model_profile="leader_main",
                      tool_bindings=["files", "shell"]),
            AgentSpec(id="coder", name="Coder", role="worker",
                      runtime_kind=RuntimeKind.DEEPAGENTS,
                      instructions="修复失败测试；只改必要文件；改完运行 pytest 自证。",
                      model_profile="coding", tool_bindings=["files", "shell"],
                      workspace_policy="git_worktree"),
        ],
        channels=[ChannelSpec(source="leader", targets=["coder"], mode=ChannelMode.TASK),
                  ChannelSpec(source="leader", targets=["coder"], mode=ChannelMode.MESSAGE),
                  ChannelSpec(source="coder", targets=["leader"], mode=ChannelMode.MESSAGE)],
        shared_spaces=[{"id": "main", "readers": ["leader", "coder"],
                        "writers": ["leader", "coder"]}],
    )
    rt = await open_session(cwd=project, session_id="demo-fix", catalog=catalog,
                            initial_spec=spec)
    await rt.start()
    try:
        rt.user_message(
            "项目里的 test_calc.py 失败了。请把修复工作交给 coder（isolated/worktree），"
            "要求它运行 pytest 证明修复，然后把结论写到共享空间；"
            "你验证后把成果合并回主目录，最后 signal_done。")
        ok = await rt.settle(600)
        print("settle:", ok)
        for event in rt.store.events("demo-fix"):
            if event["kind"] in ("task_created", "task_completed", "task_blocked",
                                 "goal_done", "topology_applied", "run_progress"):
                print(event["sequence"], event["kind"], event["payload_json"][:160])
        result = subprocess.run(["git", "-C", str(project), "log", "--oneline", "-3"],
                                capture_output=True, text=True)
        print("git log:\n" + result.stdout)
        test = subprocess.run([sys.executable, "-m", "pytest", "-q"],
                              cwd=project, capture_output=True, text=True)
        print("pytest:", test.stdout.strip().splitlines()[-1] if test.stdout else test.stderr)
        return 0
    finally:
        await rt.close()
        rt.store.close()


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
