# 用户指南：配置、权限、恢复与故障处理

## 0. 版本适用性（先读）

本文档同时覆盖两个实现；标 ⚠ 的条目是 **Python（main）版专有**，Rust 重构版（`reconstruct`
分支，入口 `engine/target/debug/teamagents`）尚未移植，遇到时按“Rust 版差异”一列处理。

| 能力 | Python（main） | Rust（reconstruct 分支） |
|---|---|---|
| 用户配置 TOML | ✅ | ✅（`models`/`tools`/`skills_paths`/`instruction_files`；其余段落忽略） |
| 项目配置 `.teamagents/config.toml` | ✅ | ✅（同名用户定义优先；项目工具需 `[permissions] trust_project_tools = true`） |
| `[permissions]` 配置项 | ✅ | ✅（`mode` 与 `trust_project_tools`；`--full-auto` 仍可覆盖） |
| 模型接入 | langchain-* provider | OpenAI 兼容 HTTP + **Anthropic Messages API**（`protocol = "anthropic"`）；默认端点按 `provider`/`protocol` 解析（deepseek → api.deepseek.com/v1，anthropic → api.anthropic.com） |
| 内置工具 `files`/`shell` | ✅ | ✅ |
| `web_search`/`web_fetch` 绑定 | ✅ | ✅（`web_search` 目前只支持 `provider="anysearch"`） |
| MCP 工具服务 | ✅ | ✅ stdio 传输（http/sse ⚠ 未实现）；工具名 `<service>_<tool>`，`tool_names` 过滤，绑定即授权 |
| Skills / AGENTS.md 注入 | ✅ | ✅（内容注入系统提示词，上限 8KB/文件、32KB/成员；Python 版是虚拟文件系统） |
| `workspace_policy` | shared / isolated / git_worktree | ✅ 三者齐全（worktree 复用、脏仓库回退 shared 并说明、未合并成果拒绝清理；删除会话同样受保护） |
| 会话锁 | flock | pid 文件 + `/proc` 存活检查（语义等价） |
| TeamSpec 导入 | JSON / YAML | JSON / YAML（`--team` 与 `validate` 均可） |
| TUI | Textual | ratatui（面板与键位一致，见文末键位表） |
| deepagents 子代理 / 图框架 | ✅ | ⚠ 未移植（`ChatRunner` 工具循环取代；`general-purpose` 子代理没有等价物） |

## 1. 配置

### 1.1 位置

| 内容 | 位置 |
|---|---|
| 用户配置（模型 profile、工具绑定、Skills 目录、指令文件） | `$XDG_CONFIG_HOME/teamagents/config.toml`（默认 `~/.config/teamagents/config.toml`） |
| ⚠ 项目配置（Python 版；可覆盖同名模型/工具，不能开启全自动或扩大预授权） | `<项目>/.teamagents/config.toml` |
| 会话状态（业务库、成员私有检查点、制品、成员工作目录） | `$XDG_STATE_HOME/teamagents/sessions/<session_id>/` |
| TUI 语言与动效偏好 | `$XDG_STATE_HOME/teamagents/ui.json`（默认 `~/.local/state/teamagents/ui.json`） |
| TUI 输入历史 | `$XDG_STATE_HOME/teamagents/composer-history.json`（上限 500 条，跨会话与重启保留） |
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

接入（Python 版）：OpenAI 用 `langchain-openai`，Anthropic 用 `langchain-anthropic`，
DeepSeek 用 `langchain-deepseek`；Kimi/GLM 走 OpenAI 兼容路径（填 `base_url` 与模型名即可）。
Rust 版只走 OpenAI 兼容 HTTP：省略 `base_url` 时按 `provider`/`protocol` 取默认端点
（`deepseek` → `https://api.deepseek.com/v1`，其他 → `https://api.openai.com/v1`），
因此第三方服务要么与这两者同源，要么显式填 `base_url`。

### 1.3 工具绑定（绑定即授权）

```toml
[tools.web]                    # 网页搜索（AnySearch HTTP；也可换成 Tavily 等 MCP 服务）
kind = "web_search"
provider = "anysearch"
url = "https://api.anysearch.com/v1/search"
api_key_env = "ANYSEARCH_API_KEY"

[tools.fetch]
kind = "web_fetch"

[tools.notes]                  # ⚠ Python 版：任意 MCP 服务（stdio 或 HTTP）——Rust 版尚未实现
kind = "mcp"
mcp_server = "notes"
mcp_transport = "stdio"
command = "npx"
args = ["-y", "some-mcp-server"]
required = false               # 必需服务不可用会明确阻塞；可选服务失败只丢该能力
```

内置能力名 `files` / `shell` / `web` 不需要配置条目：成员在 TeamSpec 里引用即可。

### 1.4 Skills 与指令文件（⚠ Python 版；Rust 版尚未移植）

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

切换：TUI `Ctrl+F`，或 `teamagents --full-auto`，或（⚠ Python 版）用户配置
`[permissions] mode = "full_auto"`。项目配置**不能**开启全自动。
Rust 版以会话行为准：审批门每次调用前读取会话的权限模式，所以切换立即生效、无需重开。

### 2.2 批准语义

- 批准绑定**具体操作与参数**；参数变化需要重新批准。
- 提供三种决定：本次批准、会话内批准（同操作哈希复用）、拒绝。
- 等待批准只暂停相关操作，其他成员继续；`WAITING_APPROVAL` 不算回合结束。
- 恢复时重新核对参数、配置版本与权限，历史批准不会沿用失效范围。

### 2.3 隔离边界（诚实说明）

- Shell 走 bubblewrap：只挂载系统只读目录 + 授权工作目录，隔离 PID/网络/临时目录；
  网络默认关闭，需要联网的操作要批准。Rust 版**要求** bwrap：缺失时命令直接失败
  （`IsolationUnavailable`），不会退化成不隔离执行；命令环境是白名单（不含模型密钥）。
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
├── checkpoints.sqlite ⚠ Python 版：成员私有线程（LangGraph 检查点）
├── artifacts/         长输出与制品（工具结果里的 /artifacts/xxx 指向这里）
├── members/<成员>/work/  ⚠ Python 版：隔离/worktree 成员的专属工作目录
├── workspaces/<成员>/   Rust 版：workspace_policy=isolated 成员的工作目录（文件工具的执行根）
└── session.lock       执行所有权文件（同一会话同时只允许一个运行实例；Rust 版记 pid）
```

```bash
teamagents sessions            # 列出会话：状态、目标、事件/任务数、占用空间、路径
teamagents --resume <会话 id>   # 恢复该会话（团队版本、待办、消息位置、成员线程、批准队列）
```

TUI 里同一件事在「会话」面板完成（`Ctrl+T` 循环到该面板）：
`s` 或 `Enter` 切换、`n` 在当前目录新建会话、`a` 归档、`d` 删除（连按两次确认）。
**归档/删除的是当前会话时会直接退出 TUI**；其他会话操作后留在原地并刷新列表。
运行中的会话（其他进程持有文件锁）不允许切换/归档/删除，会明确告知
（Rust 版提示 `session <id> is already running (pid <n>)`）。

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
| 隔离或协议自检 | `teamagents doctor`（依赖、配置、bubblewrap、codex、状态目录） |
| Rust 版提示“找不到 teamagents-tui” | 先 `cd tui && cargo build`；或用 `TEAMAGENTS_TUI=/路径/teamagents-tui` 指定 |
| Rust 版成员命令报 `IsolationUnavailable` | 未安装 bubblewrap；装上再试（不要用降低隔离的方式绕过） |
| Rust 版 `--team` 报 `bad spec` | TeamSpec 需为 JSON 或 YAML；`examples/team.yaml` 可直接使用 |

## 5. 团队定义（TeamSpec）要点

- 字段：`leader_id`、`agents[]`（id/name/role/runtime_kind/instructions/model_profile/
  tool_bindings/skills/workspace_policy）、`channels[]`、`observers[]`、`shared_spaces[]`、`limits`。
- 校验：Leader 唯一且存在、成员 ID 唯一、引用有效、任务依赖无环、Codex 成员只能由 Leader 委派、
  上限为正数；通过 `teamagents validate` 可离线检查。
- 工作目录策略：`shared`（同一目录）、`isolated`（成员目录 + 明确输入/制品引用）、
  `git_worktree`（从明确提交建分支与 worktree；原目录脏时自动退回 shared 并说明原因）。
  ⚠ Rust 版实现 `shared`/`isolated`；`git_worktree` 成员会明确失败
  （`workspace_policy=git_worktree is not implemented…`），不会静默按 shared 运行。

## TUI（Rust 版布局与交互）

> Python（main）版是 Textual 界面，布局为"上管理区 + 下对话区"，键位与本表基本一致；
> Rust 版按 D-20 重新设计，以终端习惯为准（详见 `docs/DECISIONS.md` D-20）。

- **布局自适应**：终端宽度 ≥110 列时为"左侧聊天主区 + 右侧管理栏（圆角框）"——管理栏从终端
  **第一行起**、底边与输入框下边框**对齐**，第一行左侧是状态行；更窄时自动堆叠为
  "状态行 + 上管理栏 + 下聊天"。**管理栏宽度按当前面板的内容伸缩**（表格按列宽合计、日志 84 列、
  设置按最长设置行），并保证聊天区至少 44 列——所以切换面板时右栏宽度会随之变化。
  管理栏含七个页签（团队/任务/共享空间/批准/会话/日志/设置），页签上直接显示计数
  （如 `Tasks 2`、`Approvals 1`），选中页签用 `▍` 标出；页签栏**始终一行**，
  放不下时以当前页签为中心滚动显示，两端出现 `‹`/`›` 表示还有页签被折叠。
- **状态行**：`TeamAgents「应用名」+ 会话 id + 权限模式胶囊（全自动为强调色底）+ 待批准数量 +
  未完成任务数量`，暂停时插入警示胶囊；宽度不够时按优先级丢弃末尾胶囊而不是截断文字。
- **活动行**：正在执行的成员以胶囊显示（`⠸ leader 00:12`，含已用时间），右侧灰字显示"最近活动"；
  无执行时显示 `○ Ready`。
- **对话区**：底部对齐、可滚动，右侧细滚动条；上翻时右下角提示"已上翻 N 行 · Ctrl+End 回到底部"。
  Leader 的流式回复带强调色竖条预览，回合结束后归档为正式消息。新提交会把视图拉回最新。
- **作曲家**：圆角输入框，聚焦时边框变强调色；空输入显示占位提示；上边框右侧显示 Leader 状态与模型，
  下边框显示按键提示（按可用宽度自适应）。
- **管理栏表格**：表头暗色带分隔线、极淡斑马纹、选中行左侧 `▌` 强调条；列为自适应，
  窄栏会依次收起次要列（团队表的 Type/Workspace/Access、任务表的 Created/Result 等）。
  空列表显示"— No tasks yet —"之类的提示。
- **长列表**：表格行数多于可视高度时随选中行滚动（选中行优先居中），右侧出现细滚动条。
- **鼠标**：滚轮悬停在管理栏上时等同 ↑/↓ 移动表格选中行；在聊天/日志上时滚动内容；
  点击页签切换面板；点击表格行选中；点击聊天区回到输入框。

| 操作 | 快捷键 |
|---|---|
| 发送 / 换行 | Enter / Shift+Enter 或 Ctrl+J |
| 调取输入历史 | 首行按 ↑，末行按 ↓；返回最新位置恢复草稿；历史跨会话与重启保留 |
| 行首 / 行尾 | Ctrl+A / Ctrl+E |
| 输入框 / 管理面板 | Ctrl+N / Ctrl+T（面板内 Esc 返回输入框） |
| 批准队列 | Ctrl+G |
| 请求停止 Leader | Esc；成员的其他工作继续 |
| 滚动对话 / 日志 | PgUp/PgDn 或 Ctrl+U/Ctrl+D，滚轮同样可用；Ctrl+Home/Ctrl+End 跳到最早/最新 |
| 按词编辑 | Ctrl+W 或 Alt+Backspace 删词；Ctrl+←/Ctrl+→ 按词移动光标 |

原生成员在模型/工具边界响应停止。已经执行中的工具要先返回，界面显示停止请求；超过确认时限会显示结果不明，不承诺回滚文件或外部操作。

任务进入 BLOCKED 会唤醒等待者处理。Leader 可用 `cancel_task` 结清无需继续的任务，再按需求重新委派；用户也可在上区「任务」选中任务按 `c`。重新创建同名成员必须使用新的成员 ID，原身份不能复用。
