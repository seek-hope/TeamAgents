# 实施状态（2026-09-12）

基准：`TeamAgents-Implementation-Plan.zh-CN.md`。偏离均记于 `docs/DECISIONS.md`。

## 测试

```bash
.venv/bin/python -m pytest tests/ -q            # 确定性套件（脚本化成员）170 passed
.venv/bin/python -m pytest tests/ -q -m live    # 真实服务套件 12 passed（约 90s）
```

按方案 §17「真实 API 测试独立运行」：`live` 标记的用例默认不跑，需显式 `-m live`；
缺密钥时显式 skip，skip 不计入验收。

## 阶段状态

| 阶段 | 状态 | 证据 |
|---|---|---|
| P0 接口验证 | ✅ | `docs/P0-findings.md`；deepagents 0.7.13 / langgraph 1.2.11 / codex-cli 0.154.0 / bwrap 0.12.0 实测 |
| P1 团队语义 | ✅ | T1–T6、违规拒绝、资源上限（`tests/test_t*.py`、`test_p1_guards.py`）|
| P2 持久运行 | ✅ | T8/T21 崩溃窗口、取消/暂停（`test_p2_*.py`）|
| P3 内置成员与模型 | ✅ | 真实 DeepAgents 图 + 22 个工具、批准 interrupt/resume、MCP、Skills/AGENTS.md、AnySearch 搜索/抓取、DeepSeek/Kimi 契约（`test_p3_*.py`）|
| P4 Leader 与动态团队 | ✅ | 单人成队/自然语言组队（实时）/成员提案由 Leader 决策/边界等待后生效/移除移交 Leader 且成果保留/换模型下一回合生效/版本冲突不半应用/执行中补充（`tests/test_p4_topology.py` + 实时组队） |
| P5 Codex 与工作目录 | 🔶 | Codex 成员实测（批准/取消/进度/恢复映射，`tests/test_p5_codex_adapter.py` + `tests/test_p5_live_codex.py`）：真实 CLI 委派→批准→建文件→SUCCEEDED；shared/isolated/worktree 已实现并测（`test_p5_workspace.py`）；仍缺：Codex 断线恢复的实时用例、worktree 真实合并的端到端用例 |
| P6 完整 TUI | ✅ | Textual 界面：对话（流式增量合并）、团队/任务/共享空间/批准/日志/设置面板、快捷键、窄屏单栏、多行中文、批准快捷键决定、权限模式切换（`tests/test_p6_tui.py`，T20）；真实模型 + 真界面 `tests/test_p6_tui_live.py` |
| P6 补充 | 回合失败/取消/成员状态/待等事件全部渲染进对话视图；启动自检提示缺失的模型 profile 与密钥；退出界面会关闭运行时（否则 aiosqlite 线程会让 `uv run` 不返回） |
| P6 会话管理 | 「会话」面板：同目录多会话切换/新建/归档/删除（删除当前会话即退出）；文件锁拒绝并行实例；成员 worktree 有未合并成果时拒绝删除（`tests/test_p6_sessions_ui.py`） |
| 默认模型 | `leader_main`/`research`/`coding` 全部 `deepseek-flash`（provider=deepseek，默认 reasoning_effort=max，测试用 high）；不支持 xhigh 的模型自动映射到 max，见 DECISIONS D-8 |
| P7 发布验收 | 🔶 | wheel/sdist 构建 + 全新 venv 安装冒烟通过；`doctor` 完备；README 与 `docs/USER-GUIDE.md`（配置/权限/恢复/故障）；三类端到端示例真实跑通（`examples/e2e_*.py`）；验收对照表 `docs/ACCEPTANCE.md`。**唯一未通过项**：Anthropic / GLM / OpenAI 官方缺有效密钥（T7 剩 3 家，测试已就绪） |

## 可运行的今天

```bash
# 1) 配置（示例见 examples/config.toml；密钥只放环境变量）
export DEEPSEEK_API_KEY=... KIMI_API_KEY=... ANYSEARCH_API_KEY=... APEXIN_API_KEY=...
cp examples/config.toml ~/.config/teamagents/config.toml

# 2) 自检
.venv/bin/python -m teamagents doctor

# 3) 开始会话（完整 TUI；纯终端环境可加 --plain）
.venv/bin/python -m teamagents --cwd /path/to/project

# 4) 校验团队定义 / 以指定团队启动
.venv/bin/python -m teamagents validate examples/team.yaml
.venv/bin/python -m teamagents --team examples/team.yaml
```

## 已知边界

- Anthropic / GLM / OpenAI 官方缺有效密钥：契约测试已写好，导出对应环境变量即可跑。
- Codex 成员的模型/provider 由成员的 `model_profile` 映射（`-c model`/`-c model_provider`），
  默认为本机 codex 默认配置；思考强度固定 `xhigh`（不支持时回退 `max`）。
- 工作目录的“合并”由 Leader 用 shell 执行（`workspace.merge_branch` 为 CLI/测试提供的同一实现），
  冲突留给 Leader 决策，清理永不丢弃未合并成果。
