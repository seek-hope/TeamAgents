"""End-to-end example 3: 文件/数据整理并交付制品（真实模型）。

    DEEPSEEK_API_KEY=... python examples/e2e_data_cleanup.py [目录]
"""

from __future__ import annotations

import asyncio
import csv
import os
import random
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from teamagents.models import (AgentSpec, ChannelMode, ChannelSpec, ModelProfile,
                               RuntimeKind, TeamSpec, UserConfig)
from teamagents.session import open_session


def seed_data(root: Path) -> Path:
    data_dir = root / "data"
    data_dir.mkdir(parents=True, exist_ok=True)
    random.seed(7)
    with (data_dir / "sales.csv").open("w", newline="") as fh:
        writer = csv.writer(fh)
        writer.writerow(["region", "units", "price"])
        for region in ("north", "south", "east", "west"):
            for _ in range(20):
                units = random.choice(["3", "5", "", "7", "abc"])
                price = random.choice(["12.5", "9.99", "", "10"])
                writer.writerow([region, units, price])
    return data_dir


async def main() -> int:
    if not os.environ.get("DEEPSEEK_API_KEY"):
        print("需要 DEEPSEEK_API_KEY")
        return 2
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(tempfile.mkdtemp(prefix="ta-data-"))
    data_dir = seed_data(root)
    os.environ.setdefault("XDG_STATE_HOME", str(root / "state"))
    catalog = UserConfig(models={
        "leader_main": ModelProfile(provider="deepseek", protocol="deepseek",
                                    model="deepseek-flash",
                                    api_key_env="DEEPSEEK_API_KEY")})
    spec = TeamSpec(
        leader_id="leader",
        agents=[AgentSpec(id="leader", name="Leader", role="leader",
                          runtime_kind=RuntimeKind.DEEPAGENTS,
                          instructions="整理数据并交付制品；用 signal_done 交付。",
                          model_profile="leader_main",
                          tool_bindings=["files", "shell"])],
        shared_spaces=[{"id": "main", "readers": ["leader"], "writers": ["leader"]}],
    )
    rt = await open_session(cwd=root, session_id="demo-data", catalog=catalog,
                            initial_spec=spec)
    await rt.start()
    try:
        rt.user_message(
            f"data/sales.csv 有脏数据（空值和 'abc'）。请：1) 写出清洗后的 "
            "clean.csv（只保留合法数字行，units/price 为整数/小数）；"
            "2) 生成 summary.md：每个 region 的 units 合计与 price 均值；"
            "3) 把产物路径写进共享空间 main；4) signal_done。")
        ok = await rt.settle(600)
        print("settle:", ok)
        for name in ("clean.csv", "summary.md"):
            candidates = [root / name, root / "data" / name]
            path = next((p for p in candidates if p.exists()), None)
            print(f"{name}: {'存在 -> ' + str(path) if path else '缺失'}")
            if path is not None:
                print(path.read_text()[:300])
        for entry in rt.store.shared_entries("demo-data", ["main"]):
            print("共享空间：", entry.content[:200] or entry.ref)
        return 0
    finally:
        await rt.close()
        rt.store.close()


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
