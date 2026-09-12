# 用户指南：配置、权限、恢复与故障处理

## 1. 配置

### 1.1 位置

| 内容 | 位置 |
|---|---|
| 用户配置（模型 profile、工具绑定、Skills 目录、指令文件） | `$XDG_CONFIG_HOME/teamagents/config.toml`（默认 `~/.config/teamagents/config.toml`） |
| 项目配置（可覆盖同名模型/工具，不能开启全自动或扩大预授权） | `<项目>/.teamagents/config.toml` |
| 会话状态（业务库、成员私有检查点、制品、成员工作目录） | `$XDG_STATE_HOME/teamagents/sessions/<session_id>/` |
| 团队定义导入/导出 | 任意路径的 JSON/YAML，`teamagents validate` 校验 |

### 1.2 模型 profile

```toml
[models.leader_main]
provider = "deepseek"          # 逻辑名，也用于 codex 成员的 provider 映射
protocol = "deepseek"          # openai | anthropic | deepseek
model = "deepseek-flash"       # 首版默认模型
api_key_env = "DEEPSEEK_API_KEY"          # 只引用环境变量，密钥不进仓库
timeout = 120
max_retries = 2
generation_options = { reasoning_effort = "max" }     # 默认档位；嫌慢改 high（实测 high≈10s / max≈240s）
```

推理档位规则：模型不支持 `xhigh` 时，配置里的 `xhigh` 会自动映射为 `max`
（DeepSeek 这类已知不支持的在构建模型时就映射；其他供应商在被拒绝后自动改判 `max` 重试一次）。

接入：OpenAI 用 `langchain-openai`，Anthropic 用 `langchain-anthropic`，
DeepSeek 用 `langchain-deepseek`；Kimi/GLM 走 OpenAI 兼容路径（填 `base_url` 与模型名即可）。

### 1.3 工具绑定（绑定即授权）

```toml
[tools.web]                    # 网页搜索（AnySearch HTTP；也可换成 Tavily 等 MCP 服务）
kind = "web_search"
provider = "anysearch"
url = "https://api.anysearch.com/v1/search"
api_key_env = "ANYSEARCH_API_KEY"

[tools.fetch]
kind = "web_fetch"

[tools.notes]                  # 任意 MCP 服务（stdio 或 HTTP）
kind = "mcp"
mcp_server = "notes"
mcp_transport = "stdio"
command = "npx"
args = ["-y", "some-mcp-server"]
required = false               # 必需服务不可用会明确阻塞；可选服务失败只丢该能力
```

内置能力名 `files` / `shell` / `web` 不需要配置条目：成员在 TeamSpec 里引用即可。

### 1.4 Skills 与指令文件

```toml
skills_paths = ["~/.agents/skills", "~/.config/teamagents/skills"]
instruction_files = ["~/.config/teamagents/AGENTS.md"]
```

项目根的 `AGENTS.md` 会自动作为指令文件加载。Skills 按“用户级 → 项目级 → 成员级”加载，
后者覆盖同名。Skills 不授予任何新权限。

## 2. 权限

### 2.1 两种模式

- **approved_scope（默认）**：预授权范围内自动执行；范围外生成批准请求。
  初始预授权 = 当前工作目录读写、被绑定的工具（文件/搜索/MCP）、**无网络**的隔离 Shell。
- **full_auto（仅用户可开启）**：跳过逐次批准，仍保留团队通信 ACL、动作校验、记录与执行上限；
  不绕过操作系统与外部服务的限制。TUI 状态栏始终显示当前模式。

切换：TUI `Ctrl+F`，或 `teamagents --full-auto`，或用户配置 `[permissions] mode = "full_auto"`。
项目配置**不能**开启全自动。

### 2.2 批准语义

- 批准绑定**具体操作与参数**；参数变化需要重新批准。
- 提供三种决定：本次批准、会话内批准（同操作哈希复用）、拒绝。
- 等待批准只暂停相关操作，其他成员继续；`WAITING_APPROVAL` 不算回合结束。
- 恢复时重新核对参数、配置版本与权限，历史批准不会沿用失效范围。

### 2.3 隔离边界（诚实说明）

- Shell 走 bubblewrap：只挂载系统只读目录 + 授权工作目录，隔离 PID/网络/临时目录；
  网络默认关闭，需要联网的操作要批准。
- 文件工具做符号链接与路径穿越防护，越界即拒绝。
- “私有上下文隔离”是运行时投递与工具授权合约；full_auto 允许程序按当前用户权限访问主机，
  不能同时承诺对恶意同用户进程的强保密隔离。

## 3. 恢复

- 正常退出默认保存并暂停；异常退出后下次启动自动恢复：团队版本、待办任务、消息位置、
  成员私有线程与批准队列都会重新装载。
- 执行意图（`QUEUED` 回合）先持久化再执行，恢复后继续；正在执行且无外部线程的回合可安全重跑，
  有外部线程（Codex）的回合先核对历史。
- **四类崩溃窗口**都有处理：提交后尚未启动、模型已完成但结果未归档、团队动作已提交但回执未落、
  外部工具已执行但结果未知。最后一种进入 `OUTCOME_UNKNOWN`，**不会**被当作成功或自动重试。
- 取消是“等待停止确认”，不是回滚：已产生的文件、请求与制品保留；等待中的批准失效后需重新批准。

### 3.1 会话记录在哪里、怎么管

```
$XDG_STATE_HOME/teamagents/sessions/<会话 id>/     # 默认 ~/.local/state/teamagents/sessions/
├── team.db            业务事实：动作回执、事件流、任务、回合、投递、批准、共享条目、拓扑补丁
├── checkpoints.sqlite 成员私有线程（LangGraph 检查点，按 会话+成员+上下文代次 命名）
├── artifacts/         长输出与制品（工具结果里的 /artifacts/xxx 指向这里）
├── members/<成员>/work/  隔离/ worktree 成员的专属工作目录
└── session.lock       执行所有权文件锁（同一会话同时只允许一个运行实例）
```

```bash
teamagents sessions            # 列出会话：状态、目标、事件/任务数、占用空间、路径
teamagents --resume <会话 id>   # 恢复该会话（团队版本、待办、消息位置、成员线程、批准队列）
```

TUI 里同一件事在「会话」面板完成（`Ctrl+T` 循环到该面板）：
`s` 或 `Enter` 切换、`n` 在当前目录新建会话、`a` 归档、`d` 删除（连按两次确认）。
**归档/删除的是当前会话时会直接退出 TUI**；其他会话操作后留在原地并刷新列表。
运行中的会话（其他进程持有文件锁）不允许切换/归档/删除，会明确告知。

- **默认会话 id** 由工作目录派生（`proj_<12位哈希>`），即“一个项目一条会话线”；换目录或 `--resume` 指定其它会话即为隔离的新会话，互不继承（T6/T24）。
- **删除**：删掉对应会话目录即可；若成员用过 `git_worktree`，先在项目里 `git worktree remove <路径>`（未合并成果要先处理，见 §5 与 `workspace.py`）。
- **导出/排查**：`team.db` 是普通 SQLite，可直接查，例如查看最近事件：
  ```bash
  sqlite3 ~/.local/state/teamagents/sessions/<id>/team.db \
    "select sequence,kind,actor_id,substr(payload_json,1,120) from events order by sequence desc limit 20;"
  ```
- **不记录什么**：模型密钥（只引用环境变量）、Codex 自身的会话历史（在 `~/.codex`，我们只存线程引用）、TUI 的界面状态（展开/光标等）。
- **并发保护**：会话文件锁 + SQLite 单写入口；第二个进程会明确报 “session is already running in another process”。

## 4. 故障处理

| 现象 | 处理 |
|---|---|
| 界面提示“成员 leader 的模型 profile 'leader_main' 未配置” | 会话启动时即提示（TUI 会直接写出配置路径）。创建 `~/.config/teamagents/config.toml`（可复制 `examples/config.toml`），补 `[models.leader_main]` 后重开会话 |
| 输入后长时间没有回应 | 先看对话视图：回合失败会以 `✗ …回合失败：<原因>` 显示；若没有该行且状态栏“活动回合 ≥1”，说明模型正在生成（xhigh 思考可能较慢）。最常见原因是模型 profile 未配置或密钥环境变量缺失（`doctor` 可确认） |
| 报缺少某环境变量 | profile 的 `api_key_env` 指向的变量未导出；导出后重跑 |
| 命令因“refusing to run without isolation”失败 | 安装 bubblewrap；不要以降低隔离来绕过 |
| Codex 成员卡在“Reconnecting” | codex 的 provider 凭据不可达：检查 `~/.codex/config.toml` 的默认 provider 与密钥，或给成员配置走环境变量密钥的 profile |
| 回合因 `LIMIT_REACHED` 停止 | 达到目标回合/步骤上限；调整 `limits` 后继续，不是失败终态 |
| 任务长期 `BLOCKED` | 依赖失败或成员回合未提交完成申请；Leader 会收到事件，可在 TUI 里取消或重派 |
| 需要查看发生了什么 | TUI 日志面板 / `sessions/<id>/team.db` 的 events 表 / `run_progress` 事件 |
| 隔离或协议自检 | `teamagents doctor`（依赖、配置、bubblewrap、codex schema、状态目录） |

## 5. 团队定义（TeamSpec）要点

- 字段：`leader_id`、`agents[]`（id/name/role/runtime_kind/instructions/model_profile/
  tool_bindings/skills/workspace_policy）、`channels[]`、`observers[]`、`shared_spaces[]`、`limits`。
- 校验：Leader 唯一且存在、成员 ID 唯一、引用有效、任务依赖无环、Codex 成员只能由 Leader 委派、
  上限为正数；通过 `teamagents validate` 可离线检查。
- 工作目录策略：`shared`（同一目录）、`isolated`（成员目录 + 明确输入/制品引用）、
  `git_worktree`（从明确提交建分支与 worktree；原目录脏时自动退回 shared 并说明原因）。
