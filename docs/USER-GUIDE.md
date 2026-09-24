# TeamAgents 使用指南（v2）

本指南只描述当前实现（R2 重构后的 v2 运行时）。旧产品（TeamSpec 成员、Codex 成员、`--plain` 行模式、
旧会话恢复）已随 v1 后端退役，历史说明归档在 [docs/archive/](archive/README.md)。

## 1. 快速开始

```bash
teamagents init                      # 写配置（保留既有）并准备 v2 状态根
export DEEPSEEK_API_KEY=...          # 按配置里的 api_key_env 设置
teamagents doctor                    # 检查配置、密钥、v2 状态根、bubblewrap 与本机条件
teamagents                           # 打开 TUI（必要时自动拉起当前用户的 daemon）
```

- **一个用户一个 daemon**：`teamagents` 会探测 `$XDG_STATE_HOME/teamagents/v2/daemon.sock`，不存在或连接
  被拒时以分离方式启动 `teamagents daemon`，然后把 socket 交给 TUI。退出 TUI 不会停止会话。
- **无头用法**：`teamagents exec [--json] [--timeout SEC] "提示词"` 走同一个 daemon，输出目标终态、
  助手回复或超时；退出码 0 表示已结算。
- 状态根：`$XDG_STATE_HOME/teamagents/v2/`（默认 `~/.local/state/teamagents/v2`），
  其中 `session.sqlite` 是**唯一权威状态**（WAL + `synchronous=FULL`，带格式/版本印记）。

## 2. 配置

用户配置 `$XDG_CONFIG_HOME/teamagents/config.toml`（默认 `~/.config/teamagents/config.toml`）。
密钥只写环境变量名，不写进文件：

```toml
[models.leader_main]
provider = "deepseek"
protocol = "deepseek"        # deepseek | chat/completions | responses | anthropic
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
context_window = 1000000     # 原生窗口；未知时留空（不得随手填小值，D-36）

[tools.web]                  # 可选：工具绑定（web / fetch / mcp）
kind = "web_search"
provider = "anysearch"
url = "https://api.anysearch.com/v1/search"
api_key_env = "ANYSEARCH_API_KEY"
```

- `context_window` 决定请求预算与压缩阈值；真实模型必须使用原生窗口并记录来源（见 `docs/DECISIONS.md` D-36）。
- `teamagents doctor` 会逐项检查配置、每个模型 profile 的凭据可解析、v2 状态根（印记/WAL/读写）、
  bubblewrap 隔离探针与 `[hooks]` 里的程序是否可执行；旧版 `sessions/` 布局存在时会明确报告（不迁移）。

### 2.1 钩子（`[hooks]`）

钩子是**你自己写的程序**（路径只来自用户配置，模型无法指定），在主机上以你的权限运行：

```toml
[hooks]
notify = ["/home/you/bin/teamagent-notify.sh"]    # 事件通知：argv[1] 是事件名，事件 JSON 走 stdin
pre_tool = ["/home/you/bin/policy.sh"]            # 工具调用前的策略钩子
```

- `notify`：异步、不阻塞回合，超过 10 秒被杀掉；失败只写引擎 stderr。v2 的事件为 `tool_call`
  （带 `tool`/`arguments`/`ok`/`error`）、`team_action`、`run_completed`、`run_failed`、
  `run_cancelled`、`run_paused`（实例进入 PAUSED）。
- `pre_tool`：每个原生工具调用（文件/Shell/web/Skills/MCP）执行前同步调用。退出码 0 放行；
  **退出码 2 拒绝**，该次调用的第一行 stderr 作为原因回给模型；其他退出码、启动失败或超时一律放行
  并记 stderr——坏掉的钩子不会卡死团队。崩溃恢复后的重放不再重问（决定在首次派发时做过），
  `[checks]` 里的必需检查是用户自己的验收命令，不经过 `pre_tool`。

## 3. 会话与团队

- 会话由 daemon 拥有：一个 `/v1` 协议（JSON 行）的 Unix socket，TUI 与 `exec` 都是薄客户端；
  断线后按事件水位续读，命令按 `command_id` 去重。
- **团队由 Leader 建立**：Leader 可用 `spawn` 建工作实例、`delegate` 派任务、`send` 发消息、
  `wait` 等待结果；这些能力按授权（`manage`/`delegate`/`message`）出现在模型可见工具面，
  派发时在控制平面重查权限版本。
- **工作区策略**：`spawn` 的 `workspace` 参数决定新实例在哪里干活——`shared`（默认，项目目录）、
  `isolated`（会话状态根下的私有目录）或 `git_worktree`（自己的分支与 worktree）。请求 worktree 但项目
  有未提交改动或不是 Git 仓库时，按共享模式执行并在工具回执里说明原因；实例终止时按记录回收，**存在
  未提交或未合并成果的目录不会被自动删除**，只报告原因。
- 用户侧干预：TUI 里切换实例、暂停/恢复/取消、批准或拒绝工具请求；额度、任务与授权面板同源。
- 任务与目标的完成由运行时的完成检查闸门把关：`finish` 只接受诚实结论，用户/项目预定义的
  必要检查必须真实通过。

## 4. 权限与隔离

| 模式 | 行为 |
|---|---|
| `approved_scope`（默认） | Shell 走 bubblewrap 隔离：网络与越界写入需要用户批准；批准绑定具体操作与参数散列 |
| `full_auto` | 主机 Shell（D-41）：长命令与后台服务可跨调用存活，`exec` 退出不影响已启动服务 |

bubblewrap 不可用时启动失败是**分类错误**（`started=false`），不会静默回退到主机执行。

## 5. Skills 与 MCP

- Skills 注册根：`~/.agents/skills`（`skill search/read` 按需检索；技能指令不能扩大执行权限）。
- MCP：绑定服务在启动时加载（required 失败公开报错，optional 只丢能力）；调用经统一权限、批准、
  预算、取消与回执入口；崩前已派发的远端调用恢复后记为 `OUTCOME_UNKNOWN`，**绝不重放**。

## 6. 恢复、压缩与清理

- 崩溃恢复按持久化位置分类：已知结果复用、在途丢失诚实记账（`OUTCOME_UNKNOWN`）、不猜测重放；
  磁盘满时停止新的副作用派发并停放，报告在途损失（`A31`）。
- 长上下文按**实际窗口占用**触发压缩：摘要保留原始要求、用户修订、验收与未决问题，原文经
  `read_history` 仍可检索；压缩调用计入目标预算。
- 数据清理（§14）：`python3 review/r28-legacy-cleanup.py`（先出清单，`--apply` 才删除）；
  旧版状态已清理，凭据、`~/.agents/skills`、`~/.codex` 与 `review/` 证据一律保留。

## 7. 常见问题

| 现象 | 处理 |
|---|---|
| `exec: connect ... Connection refused` | 先 `teamagents doctor` 看 v2 根；daemon 会由下一条 `teamagents`/`exec` 自动拉起 |
| `doctor` 报 v2 state root FAIL | 该路径下不是 v2 会话库（印记不符）；换一个 `--state-root` 或按提示处理，勿手工改库 |
| 模型报 401/402 | 检查对应 `api_key_env` 环境变量；`doctor` 会列出每个 profile 的凭据解析结果 |
| `approved_scope` 下命令一直等批准 | 在 TUI 批准面板处理；或改用 `--full-auto`（主机执行，D-41） |
| 想彻底重来 | 停掉 daemon，`teamagents --state-root <新目录>` 另起一个干净会话（旧库留在原位） |
