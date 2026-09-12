# TeamAgents

运行在 Linux 终端上的**团队式 Agent 产品**：你只和 Leader 说话，Leader 按需组队、委派、
协调多个成员（内置 Deep Agents 成员 + 本机 Codex 执行成员）完成目标。
团队结构、通信权限、观察权限都是**运行时校验的数据**，不是提示词约定。

产品与实现的基准文档是 `TeamAgents-Implementation-Plan.zh-CN.md`；实施状态见
`docs/STATUS.md`，已确认的设计决定见 `docs/DECISIONS.md`。

## 安装

```bash
uv venv && uv pip install -e .            # 或: pip install .
```

要求：Python ≥ 3.12、Linux（bubblewrap 做命令隔离）、可选的 `codex` CLI（外部执行成员）。

## 快速开始

```bash
# 1) 配置模型与工具（示例见 examples/config.toml；密钥只放环境变量）
mkdir -p ~/.config/teamagents && cp examples/config.toml ~/.config/teamagents/config.toml
export DEEPSEEK_API_KEY=... KIMI_API_KEY=... ANYSEARCH_API_KEY=...

# 2) 自检：依赖、配置、隔离、Codex 协议
teamagents doctor

# 3) 进入 TUI（默认单 Leader；也可以 --team examples/team.yaml 带外部 Codex 成员）
teamagents --cwd /path/to/project
```

TUI 上区为团队、任务、共享空间、批准、会话、日志与设置；下区为 Leader 对话和输入。
界面默认英文，在 `Settings → Interface language` 选择 `中文` 可立即切换并保存偏好。
任务按创建时间从新到旧排列；运行时显示成员动效、计时和最近活动，可在 `Settings → Animations` 关闭旋转动效。
输入区支持 ↑↓ 历史（跨会话与重启保留）与草稿恢复，`Ctrl+A`/`Ctrl+E` 移动到行首/行尾，`Esc` 请求停止 Leader。

TUI 常用键：`Enter` 发送、`Shift+Enter`/`Ctrl+J` 换行、`Ctrl+G` 批准队列、
`Ctrl+T` 切换面板、`Ctrl+P` 暂停/继续、`Ctrl+F` 全自动开关、`Ctrl+R` 刷新、`Ctrl+Q` 退出；
「会话」面板支持 `s` 切换、`n` 新建、`a` 归档、`d` 删除本目录下的会话
（归档/删除当前会话后退出）。
批准队列里 `a`=本次批准、`s`=会话内批准、`d`=拒绝。窄终端保留上下分区，上区通过标签切换。
纯终端环境可用 `teamagents --plain`（行式 REPL）。

## 其他入口

```bash
teamagents validate TEAM_SPEC     # 校验导入的团队定义（TeamSpec）
teamagents sessions               # 列出本机会话记录（状态/事件数/占用空间/路径）
teamagents --team TEAM_SPEC       # 用指定团队开启新会话
teamagents --resume SESSION_ID    # 恢复会话（团队版本、待办、消息位置、成员线程）
teamagents --full-auto            # 用户显式选择全自动模式
teamagents version                # 依赖版本
```

## 示例

```bash
DEEPSEEK_API_KEY=... python examples/e2e_project_fix.py    # 项目修改并测试（worktree 成员 + Leader 合并）
DEEPSEEK_API_KEY=... ANYSEARCH_API_KEY=... python examples/e2e_research.py "问题"   # 联网调研并附来源
DEEPSEEK_API_KEY=... python examples/e2e_data_cleanup.py   # 文件/数据整理并交付制品
```

## 测试

```bash
.venv/bin/python -m pytest tests/ -q            # 确定性套件（脚本化成员）
.venv/bin/python -m pytest tests/ -q -m live    # 真实服务套件（需要密钥/本机 codex）
```

更多：`docs/USER-GUIDE.md`（配置、权限、恢复、故障处理）、`docs/ACCEPTANCE.md`（验收对照表）。

---

## TS+Rust 重构版（本分支）

Python 原版见 `src/teamagents/`（main 分支为基准）。本分支为 TypeScript+Rust 重构：

- `core/`（Rust）：权威核心——TeamSpec 模型与校验、SQLite 存储（DDL 与 Python 逐字一致）、
  Control 事务管线（validate/reduce/schedule/finalize）、信息权限（views）。
  对外是 stdio 换行 JSON 服务 `teamagents-core`。
- `ts/`（TypeScript，零运行时依赖，Node ≥26）：运行时循环、ToolGateway/审批、
  成员后端（`ChatRunner` LLM 工具循环、`CodexRunner` app-server）、工具执行器、
  CLI 与零依赖 ANSI TUI。

```bash
cd core && cargo build && cargo test     # Rust 核心
cd ts && node --test test/               # TS 全部测试（T1–T5/T9、取消/暂停、Codex 适配、TUI 冒烟）
node ts/src/cli.ts doctor                # 自检
node ts/src/cli.ts                       # TUI
node ts/src/cli.ts --plain               # 行模式 REPL
```

进度台账与取舍：docs/RECONSTRUCT.md；决策：docs/DECISIONS.md（D-15）。
