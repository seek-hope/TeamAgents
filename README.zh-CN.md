# TeamAgents

[English](README.md) · [使用指南](docs/USER-GUIDE.md) · [设计](docs/DESIGN.md) · [验收](docs/ACCEPTANCE.md)

TeamAgents 是一个 Rust 原生、本地优先的智能体运行时。Leader 与用户协作，可以把受限任务委派给其他
实例，并在每个会话的权威 SQLite 存储中持久化执行状态、审批、上下文、用量和证据。TUI 与无界面客户端
共享同一个 daemon 和阶段机。

当前格式为 **数据库 schema 9**、**socket protocol 1** 和 **work policy 1**。

## 当前能力

- 工作可跨轮次、compact 和重启持续进行，支持有界续跑、预算、截止时间和必须的验收检查。
- 通过 `spawn`、`delegate`、`send`、`wait` 进行受限协作，支持共享目录、隔离目录或 Git worktree。
- 文件、宿主 Shell、Web 搜索/抓取、Skills、stdio/HTTP MCP resources，以及受限的 JavaScript `codemode`。
- Chat Completions、DeepSeek、Responses 和 Anthropic 配置；每个模型使用其原生上下文窗口，并可配置推理档位。
- 单一 Leader 对话，支持 Markdown/代码/媒体渲染、条件审批、待处理输入编辑、会话历史/搜索/切换/删除
  和本地附件。
- 与 compact 配合的**会话级探索树**，具有修订记录、来源引用和 focus。
- 当前开发与验证工具位于 `tools/`。

宿主 Shell、验收命令和本地 MCP 不构成操作系统沙箱。逻辑授权与私有历史规则仍然有效。未知副作用记为
`OUTCOME_UNKNOWN`，绝不凭猜测重放。

## 构建与启动

按照 `rust-toolchain.toml` 安装固定 Rust 工具链，然后运行：

```bash
make check CARGO_FLAGS=--locked   # 首次运行可能下载依赖
make build
engine/target/debug/teamagents init
export DEEPSEEK_API_KEY=...       # 使用配置中 api_key_env 指定的环境变量
engine/target/debug/teamagents doctor
engine/target/debug/teamagents
```

`teamagents` 会启动或加入会话 daemon，并打开 TUI。`init` 不会覆盖已有配置。默认模型 profile 键为
`leader_main`；使用 `--model` 选择其他 catalog 项。无界面运行一轮：

```bash
engine/target/debug/teamagents exec --json --timeout 180 "检查此仓库并概述当前状态"
```

自定义 provider、权限和存储位置见[安装与升级](docs/INSTALL.md)和[配置](docs/CONFIG.md)。

## 会话探索树

探索树以模型维护的导航视图记录问题、假设、尝试、发现、决策、阻碍和下一步。它不是已验证事实、完成证据、
权限，也不是撤销副作用的机制。

- 每个会话一棵树，以 append-only 修订和 head 指针持久化。
- Leader 的 compact 在上一棵累积树的基础上，一直梳理到最近保留的一轮对话。
- 成员的私有历史不会复制进共享树。
- 节点携带来源引用；freshness 使用完整 `(instance, epoch, entry)` watermark 比较。
- `add`、`update`、`move`、`merge`、`focus` 使用带 revision 的原子 patch。有序操作整体回滚，重放不会
  产生重复 revision。
- compact 的树 patch 无效时保留旧树为 stale，但不会阻断摘要或当前工作。

在 TUI 中输入 `/tree`。在终端中运行：

```bash
engine/target/debug/teamagents search_tree
engine/target/debug/teamagents search_tree --json
```

可以用自然语言让 Leader 合并重复分支、放弃死路、重新整理或 focus 某个分支。完整原始历史仍独立保留并可读取。

## 日常操作

常用 TUI 按键：

- `Enter` 发送，`Shift+Enter` 换行。`↑`/`↓` 回顾输入；`PageUp`/`PageDown` 和鼠标滚轮翻页。
  `Ctrl+W` 删除单词，`Ctrl+Left`/`Ctrl+Right` 按单词移动。
- `Ctrl+A` 聚焦审批列表（`a` 批准、`d` 拒绝、`Esc` 返回）。`Ctrl+C` 中断当前模型请求，不会停止
  正在运行的工具或已委派工作。`Ctrl+Q` 或 `/quit` 关闭 TUI，daemon 继续运行。
- `Ctrl+M` 或 `/model` 打开模型与推理档位菜单。`Ctrl+P` 或 `/queue` 查看待处理输入。
  `Ctrl+S` 或 `/sessions` 打开会话历史与切换。`Ctrl+E` 使用 `VISUAL` 或 `EDITOR` 编辑草稿。
- `Ctrl+O` 展开或折叠工具预览。`Ctrl+G` 在预览和源码之间切换。

斜杠命令包括 `/help`、`/sessions`、`/interrupt`、`/quit`、`/model`、`/attach`、`/detach`、
`/queue`、`/tree` 和 `/edit`。补全只填入命令，仍需按 Enter 才会提交。输入 `@PATH` 后按 Tab 并选择候选，
可以暂存文件引用；文件只会随你之后发送的消息一起提交。

默认状态根为 `${XDG_STATE_HOME:-$HOME/.local/state}/teamagents/v2`。

常用 CLI：

```bash
teamagents status --json
teamagents history --id INSTANCE --limit 400 --json
teamagents search_tree --json
teamagents instances
teamagents tasks
teamagents work_scopes
teamagents sessions
teamagents mcp list
teamagents version
```

重新连接会话时请使用相同的 `--state-root` 或 `--session`。取消任务并不能证明进程已经停止；实例终止
和共享工作区接管使用独立的停止证据。

## 架构

```text
TUI / exec / CLI
       |
       v
daemon + supervisor + 每个实例的 driver
       |
       v
有界存储 worker -> Control::submit -> SQLite
       |
       +--> 模型 provider
       +--> 工具 / Shell runner / MCP
       +--> 摘要和会话探索树
```

控制面负责身份、权限 revision 和原子团队事务。engine 负责 I/O 与阶段机。SQLite 通过 WAL 和
`synchronous=FULL` 保存权威事实。模型与工具等待发生在事务之外。TUI 只负责呈现。

状态机和数据关系见[架构指南](docs/ARCHITECTURE.md)。

## 当前证据

当前本地门禁包含 **777 项 Rust 测试**（core 191、engine 481、TUI 105），`make check` 退出 0。
形式化材料包含 **48 个模块、393 个配置**：

- **65 个正向配置，62 次遍历**；
- **327 个负向控制**均被反驳；
- **79 条正常路径 witness** 均已到达；
- **9 个 Kani harness** 全部通过。

探索树验证包含有序双操作 patch、23 个覆盖所有非 `TypeOK` 性质的控制，以及 11 条 witness。witness 证明
可达性，而不是安全性。PTY 覆盖五种终端 profile 和两种强制图形模式。

复现命令：

```bash
make check
make verify-model-all
make verify-model-counterexamples
make verify-witnesses
make verify-kani
make pty
```

## 当前限制

这些结果不证明完整的 Rust/SQLite refinement、全系统组合、wide exhaustive 完成或 Policy B 的源代码绑定
等价性。本地实时探针（`make probe-offline`、`make probe-models`）已覆盖四种线协议、异构 provider、
search/fetch、图片输入、reviewer 对抗用例、1M 原生窗口 compaction 与探索树更新、隔离 clean-host 安装/升级/回滚、
本地环回 SSH 与 Alacritty；更广泛的外部 OAuth 服务、公网主机、多发行版 clean-host、图形终端矩阵、
大规模长任务评测以及完整的按价成本比较仍待补充。单独的 wide 配置不宣称已经完成。

## 文档

| 文档 | 内容 |
|---|---|
| [使用指南](docs/USER-GUIDE.md) | 操作、快捷键、协作、工具和恢复 |
| [设计](docs/DESIGN.md) | 需求与架构约束 |
| [验收](docs/ACCEPTANCE.md) | Q1–Q19、A01–A36 的证据和未解决项 |
| [当前决策](docs/DECISIONS.md) | 当前有效规则 |
| [开发](docs/DEVELOPMENT.md) | 构建、测试、形式化检查和维护 |
| [架构](docs/ARCHITECTURE.md) | 状态机、数据和恢复图 |
| [能力](docs/CAPABILITIES.md) | 当前能力与证据矩阵 |
| [验证](verification/REPORT.md) | 形式化结果和抽象边界 |
| [形式化指南](verification/README.md) | 当前形式化材料与模块索引 |
| [产品比较](docs/PRODUCT-COMPARISON.md) | 当前未解决工作和比较义务 |
| [工具](tools/README.md) | 当前开发与验证工具 |

除本文件外，代码、文档和提交信息均使用英文。不得把凭据写入源码、提示词、事件或日志。
