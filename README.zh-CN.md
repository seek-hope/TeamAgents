# TeamAgents

[English](README.md) · [设计决策](docs/DECISIONS.md) · [架构图](docs/architecture.canvas) · [形式化规范](docs/FORMAL_SPEC.md)

TeamAgents 在本地工作区执行编程任务。每次进入工具都会新建独立会话，每个节点拥有相互隔离的会话历史（不同会话之间互不污染）。未指定接收者时，任务交给 `default`。每个阶段可以自行完成工作，也可以向子阶段分派任务。单轮结束时，如果**有且仅有一个叶节点**，则直接在该叶节点结算；若有**两个及以上叶节点**，则收敛进入树内 `summarizer` 汇总节点。用户后续在该已结算会话中输入文本时，将直接发送给该唯一叶节点或汇总节点，并将其作为新一轮子树的根节点继续执行。

## 安装与启动

Shell 沙箱支持原生 x86-64 和小端 AArch64 Linux。先用系统包管理器安装 `bubblewrap` 和 `slirp4netns`。Ubuntu 可运行 `sudo apt install bubblewrap slirp4netns`。运行时使用 `/usr/bin/bwrap`，联网助手默认为 `/usr/bin/slirp4netns`。如助手安装在其他位置，请在守护进程启动前，将 `TEAMAGENTS_SLIRP4NETNS` 设为可信助手可执行文件的绝对路径。系统须允许普通用户使用用户命名空间和网络命名空间。使用二进制压缩包或源码安装脚本均需单独安装这些依赖。

从源码安装需要 `rust-toolchain.toml` 指定的 Rust 工具链和 C 编译器（用于编译随包提供的 SQLite）。如果 TeamAgents 已在运行，先等在途工作结束，再在每个运行中的会话里执行 `teamagents stop`，然后更换程序。在源码目录运行 `cargo install --locked --path .` 或 `./install.sh`，把 Cargo 安装目录（通常是 `~/.cargo/bin`）加入 `PATH`，运行 `teamagents version` 确认安装成功。

启动会话守护进程前先选好模型服务，在需要处理的项目目录运行客户端。

**使用 ChatGPT 订阅。** 运行 `teamagents auth login`，用同一台机器上的浏览器打开终端给出的 OpenAI 登录链接，确认授权 TeamAgents 使用订阅额度，然后回到终端运行：

```sh
teamagents auth status
TEAMAGENTS_PROVIDER=chatgpt teamagents chat
```

守护进程通过 OpenAI 的[订阅 Responses API](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference) 调用模型。`TEAMAGENTS_MODEL` 指定初始模型。如果没有指定模型，则自动选择账户目录中的第一个可见模型。同一根任务的工作节点与汇报节点使用提交时锁定的模型服务。账户资格和额度由 OpenAI 决定。授权文件保存在工作区外的 `~/.teamagents-auth/`，`teamagents auth logout` 可清除本机授权。在交互会话（`chat` / TUI）中，可直接运行 `/model provider NAME [MODEL [EFFORT]]` 或在 TUI `/model` 弹窗中选择 `Provider:` 热切换模型服务而无需重启守护进程；独立执行 `TEAMAGENTS_PROVIDER=... teamagents exec` 时若与当前会话服务不一致，仍会拒绝提交以防止误扣费（可用 `/model provider ...` 切换或 `teamagents stop` 后重启）。

**自定义服务商（`providers.json` 与 `teamagents provider`）。** 可在 `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents/providers.json`（或通过 `TEAMAGENTS_PROVIDERS_FILE` 指定路径）中追加自定义 `responses`、`chat_completions` 或 `anthropic` 服务端点。配置文件**只保存凭证环境变量名**（`--api-key-env VAR`），绝不将明文 API 密钥写入磁盘：

```sh
teamagents provider add openrouter --wire chat_completions \
  --base-url https://openrouter.ai/api/v1 \
  --api-key-env OPENROUTER_API_KEY --model openai/gpt-4.1-mini
teamagents provider list                 # 查看内建与自定义服务商及就绪状态
teamagents provider show openrouter      # 查看脱敏后的端点配置
teamagents provider default openrouter   # 设为全局默认（`default auto` 恢复自动优先级）
teamagents provider remove openrouter    # 移除自定义服务商
```

**使用 API 密钥（DeepSeek Responses / OpenAI Chat Completions / OpenAI Responses / Anthropic Messages）。** 可通过 `TEAMAGENTS_PROVIDER` 选择 `deepseek`（DeepSeek Responses API）、`api`（Chat Completions）、`responses`（OpenAI Responses API）或 `anthropic`（Anthropic Messages API）：

```sh
# DeepSeek Responses（`deepseek`，配置 DEEPSEEK_API_KEY 时默认最高优先级自动选中）
# 默认模型：deepseek-flash；默认地址：https://api.deepseek.com/v1
# 通过密钥管理工具在当前环境设置 DEEPSEEK_API_KEY。
teamagents chat

# OpenAI Chat Completions（`api`）或 OpenAI Responses（`responses`）
export TEAMAGENTS_PROVIDER=responses   # 或 api
export OPENAI_BASE_URL=https://api.openai.com/v1  # 默认地址
export TEAMAGENTS_MODEL='your-provider-model-id'
# 通过密钥管理工具在当前环境设置 OPENAI_API_KEY。
teamagents chat

# Anthropic Messages（`anthropic`）
export TEAMAGENTS_PROVIDER=anthropic
export ANTHROPIC_BASE_URL=https://api.anthropic.com  # 默认地址
export TEAMAGENTS_MODEL='claude-sonnet-4-5'          # 或设置 ANTHROPIC_MODEL
# 通过密钥管理工具设置 ANTHROPIC_API_KEY（可选 ANTHROPIC_MAX_TOKENS）。
teamagents chat
```

未显式指定 `TEAMAGENTS_PROVIDER` 时，守护进程按以下优先级自动选择：只要配置了 `DEEPSEEK_API_KEY` 即优先使用 `deepseek`（默认模型 `deepseek-flash`，走 `/v1/responses`）；否则若已登录 ChatGPT 订阅则优先使用订阅；否则若仅配置了 `ANTHROPIC_API_KEY` 或 `ANTHROPIC_BASE_URL` 则选择 `anthropic`；其余情况默认 `api`。五种服务共享统一的静态工具前缀与末尾身份布局，在 `handoff` 时默认写时复制（Fork）父节点已预热历史前缀（可选 `inherit_context: false` 关闭），原生回传推理链与缓存提示（`prompt_cache_key`、Anthropic `cache_control`），并在状态中汇总 `usage` 缓存命中率。非回环 API 地址需要 HTTPS 和对应密钥（回环地址可使用 HTTP，也可不提供密钥）。在 `anthropic` 模式下，思考强度会映射为 Anthropic `thinking` 预算（`none` 关闭扩展思考）。模型请求读取超时为 600 秒（连接 30 秒、写入 60 秒），支持最多 5 次重定向，在多并发分支间共享限流退避窗口，并对瞬时网络故障及 408/429/5xx/529 状态最多重试至 5 次尝试；遇到额度耗尽会触发 30 秒熔断暂停以便各分支干净结算（可用 `continue` 恢复）；遇到 HTTP 413 / 上下文溢出或达到模型上下文窗口的 90%（可用 `TEAMAGENTS_CONTEXT_WINDOW` 覆盖）时会自动触发上下文压缩。

### 在对话中切换服务商、模型和思考强度

| 对话命令 | 作用 |
|---|---|
| `/model` | 查看下一个任务将使用的服务、模型和思考强度（TUI 中打开配置面板）。 |
| `/model providers` / `/provider` | 列出内建与自定义模型服务及其就绪状态。 |
| `/model provider NAME [MODEL [EFFORT]]` | 在运行中的守护进程上热切换当前会话的默认服务商，无需重启（`/model provider default` 恢复默认优先级；等价命令 `/provider use NAME`）。 |
| `/provider add \| show \| remove \| default` | 在对话/TUI 中追加、查看、删除自定义服务商或修改全局默认服务商。 |
| `/model list` | 查询当前服务商可用的模型 ID（ChatGPT 账户目录或 `/models` 接口）。 |
| `/model MODEL [EFFORT]` | 为当前服务商选择模型。省略强度时使用该模型的默认值。如果模型 ID 恰好也是命令名（如 `default`），可用 `/model set MODEL [EFFORT]`。 |
| `/model effort high` | 仅修改思考强度。`/model effort default` 恢复模型默认强度。 |
| `/model default` | 恢复当前服务商的默认模型和默认强度。 |

TeamAgents 接受 `none`、`minimal`、`low`、`medium`、`high`、`xhigh` 和 `max`，每个模型支持的档位由服务商决定。`default` 表示由服务商选择。服务商选择与模型设置按会话及各服务商分别保存，从下一次通过 `chat` 或 `exec` 提交任务时生效；正在运行或已提交的根任务及其子节点、修复节点、最终汇报均锁定使用各自提交时的 `(provider, model, effort)` 快照。

### 交互式 TUI 与纯文本对话

在交互式终端中运行 `teamagents` 或 `teamagents chat [--review] [--staged|--sandbox MODE]` 会直接进入空白的**新会话** TUI（历史会话保留在左侧边栏中可供查看、继续或删除）；使用管道输入、`--json` 或 `--plain` 时回退为纯文本行模式（同样默认开启新会话）。使用 `teamagents tui [--review] [--staged|--sandbox MODE]` 或 `teamagents resume [TASK_ID]` 可显式启动/恢复 TUI。当查看运行中的非历史根会话时，输入区直接发送消息会作为轮中引导指令（`steer`）注入下一轮请求；输入 `@` 可触发工作区文件模糊补全（`Tab`/`Enter` 插入 `@path`）。Summary 面板会实时显示流式思考/说明、活跃工具、Shell 输出尾部以及 `update_plan` 计划清单。TUI 中快捷键负责导航、布局与直接操作，斜杠命令负责配置与工作流控制：

| 快捷键 | 作用（导航、布局与直接条目操作） |
|---|---|
| `Tab` / `BackTab` | 补全 `/命令` 或 `@文件`，或在焦点面板（输入区、侧栏、详情检查器）间切换 |
| `Ctrl+N` | 切换到空白新会话（也可点击 `+New` 或执行 `/new`） |
| `Ctrl+B` | 显示/隐藏左侧边栏 |
| `Ctrl+P` | 打开待审批队列（`a` 批准，`d` 拒绝） |
| `Alt+1`–`5` / `1`–`5` | 切换检查器标签页（`Summary`、`Evidence`、`Tree`、`Log`、`Board`） |
| `[` / `]` | 切换侧栏视图（`Sessions` 会话 / `Agents` 智能体） |
| `a` / `d` / `r` / `x` | 批准 / 拒绝 / 回复澄清问题 / 取消运行中任务 |
| 连续两次 `Del` | 选中会话后第一次触发删除确认提示，第二次执行删除 |
| `Enter` / `Shift+Enter` | 提交新任务、引导运行中会话或继续唯一叶节点/汇总节点 / 插入换行 |
| `F1` / `Ctrl+Q` | 打开快捷键与命令参考 / 退出 TUI（守护进程继续后台运行） |

互补的 TUI 与对话斜杠命令：`/model`、`/provider`、`/to`、`/check`、`/review [on|off]`、`/review code [REF]`、`/sandbox [direct|staged|read-only]`、`/staged [on|off]`、`/plan TEXT`、`/diff [REF]`、`/steer TEXT`、`/compact`、`/skills`、`/mcp`、`/status`、`/wb`、`/new [TEXT]`、`/answer ROOT_ID TEXT`、`/clear`、`/stop`。

## 项目指令、技能与 MCP 扩展

- **项目指令（`AGENTS.md`）。** TeamAgents 会自动按层级发现仓库根目录至当前工作区的 `AGENTS.override.md` 与 `AGENTS.md`，以及 `~/.config/teamagents/AGENTS.md`（或 `~/.teamagents/AGENTS.md`），连同 Git 与环境上下文注入根任务前缀。
- **本地技能（`SKILL.md` 与 `read_skill`）。** 在 `.teamagents/skills/`、`.agents/skills/`、`~/.teamagents/skills/` 或 `~/.agents/skills/` 下放置带 YAML frontmatter（`name`、`description`）的 `<skill>/SKILL.md`。根任务会感知技能索引并在需要时调用 `read_skill` 读取完整说明（也可运行 `teamagents skills` 或 `/skills` 查看）。
- **MCP 服务（`mcp.json` 与 `teamagents mcp`）。** 可在 `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents/mcp.json`（或 `TEAMAGENTS_MCP_FILE` 指定路径）配置 Stdio 与 Streamable HTTP JSON-RPC 2.0 MCP 服务；配置文件**只保存环境变量名**（`--env VAR`、`--bearer-token-env VAR`），绝不落盘明文密钥：

```sh
teamagents mcp add docs --command npx --arg -y --arg @modelcontextprotocol/server-filesystem --arg .
teamagents mcp add remote --url https://mcp.example.com/mcp --bearer-token-env MCP_TOKEN
teamagents mcp list
teamagents mcp tools [NAME]
```

模型可通过 `mcp_list` 查看已配置工具并用 `mcp_call` 调用。

## 提交任务并验证结果

新任务默认无人值守。`exec` 默认等到最终报告或澄清问题，没有客户端等待时限；传入 `--jsonl` 可在 `exec`/`continue`/`review` 过程中实时输出按行分隔的事件流（`submitted`、`live`、`plan`、`input`、`approval`、`completed`）。新增 `review [--uncommitted|--base REF|--commit SHA]` 执行只读代码审查，`resume [TASK_ID [TEXT]]` 断线重连或续接会话，`diff [--base REF|--commit SHA]` 查看工作区变更，`steer TASK_ID TEXT` 注入轮中引导，`compact TASK_ID` 手动压缩对话历史。下面的示例要求修复脚本，并指定如何检验其输出。

```sh
teamagents exec --check 'test "$(sh greet.sh Ada)" = "Hello, Ada!"' \
  '修复 greet.sh，让它按传入的名字问候用户。'
```

**任务如何流转、协作与续接。** 活跃的 `Root` 与 `Node` 阶段可在单轮内并行返回最多 16 个原生工具调用（`read_file` 含可选 `line_numbers`、`list_dir`、`grep_files`、`read_skill`、`mcp_list`、`mcp_call`、V4A `apply_patch`、`edit_file`、`update_plan`、`shell`、`wb_read`、`wb_write`）：只读工具并发执行，带副作用工具严格按序执行。`handoff` 在同一笔事务中结束当前阶段，创建 1 至 64 个子阶段。每个子阶段都收到发送方的阶段小结和分给自己的任务。同一轮（`(root, round)`）内，每个成员最多承担一个阶段。单轮结束时，如果**只有 1 个叶节点**，则跳过汇总直接在该叶节点结算；如果有 **2 个及以上叶节点**，则收敛生成一个树内 `summarizer` 节点（`kind = "summarizer"`），只读轮次意图、叶子结果与只读 `whiteboard` 快照（严格视为未验证协作笔记）而无权使用工作区工具或读取成员私密历史。运行中的同级叶节点可通过根任务持久化的协同白板协作（`wb_read`、支持 `expected_version` CAS 或无锁 `append`/`resolve`/`delete` 的 `wb_write`，以及防死锁的 `wb_wait`）。白板条目跨续接轮次持久化保留，直到随根任务一并删除；可用 `teamagents whiteboard ROOT_ID`（`wb`）或对话命令 `/wb ...` 查看与编辑。后续在该唯一叶节点或汇总节点续接对话（`teamagents continue TASK_ID TEXT`、`teamagents resume TASK_ID TEXT` 或直接在已结算会话界面输入文本）时，会将该节点提升为新一轮子树的根节点，成员可跨轮次复用，且不设模型调用次数上限。同一轮内仍禁止跨分支唤醒父节点或同级节点。

**怎样判定验收成功。** `--check` 授权执行当前轮次你指定的原样命令。所有执行分支结束后，带检查的轮次须通过该命令才能最终成功结算（多叶节点时须先通过检查，再交由汇总节点生成报告）。检查失败时，运行时可以把诊断交给新的阶段来修复问题。**单轮内包括首次执行在内，检查最多运行十次**。未指定 `--check` 时，即使模型主动运行的命令成功，`verification` 仍为 `not_requested`。

**跟踪根任务并回答问题。** `exec --detach --json` 立即返回根任务 ID。`exec --timeout SECONDS` 只限制客户端等待时间。重新连接时使用同一会话和工作区。如果缺少必要信息，首个执行阶段可以在调用工具或分派前向你提问。`exec` 会打印问题并以非零状态码退出，收到答复后任务才会继续。例如，它询问该实现哪种问候语时，可以用根任务 ID 查看问题并作答。

```sh
teamagents status ROOT_ID --json
teamagents answer ROOT_ID '请使用完整的问候语。'
teamagents whiteboard ROOT_ID set contract:greeting '按第一个参数的名字输出问候语。'
```

`teamagents status ROOT_ID` 显示整次运行的状况。有分支时，它会用树形结构列出各阶段的状态、协同白板内容和任务 ID。在 JSON 中，`status` 表示整体状态，`stage_status` 表示首个阶段的状态，可能已是 `done`。`tree_nodes` 列出各阶段，`leaf_results` 包含叶子状态及报告，`whiteboard_seq` / `whiteboard` 展示协同白板条目，`subtree_receipts` 列出实际操作的回执摘要。用 `teamagents status TASK_ID` 查看单个阶段的完整 `evidence`。

根任务状态为 `input` 时，先读 `question`，再用该根任务的 ID 作答。在对话中可用 `/status ROOT_ID`、`/answer ROOT_ID TEXT` 和 `/wb ...`。查看正在提问的会话时直接输入文字即可作答；在已结算的会话界面直接输入文字则会把提示发给当前唯一叶节点或汇总节点继续子树（对应 CLI `continue TASK_ID TEXT`）。按 `Ctrl+N` 或输入 `/new` 可切回空白新会话，`/new TEXT` 可直接另开全新会话。

`/members` 列出智能体，`/to NAME` 选择下次提交对话任务时的接收者，`/tasks` 列出会话任务。离开对话或等待超时都不会取消任务。`cancel TASK_ID` 请求取消任务，`delete ROOT_ID` 删除已结束或空闲的会话树（同时清理其白板条目），`stop` 关闭空闲的守护进程并保留会话。

**如需逐项审批模型请求的 shell 命令或文件编辑**，使用 `teamagents exec --review '…'` 或 `teamagents chat --review`。子任务继承根任务的策略。在同一会话中用 `teamagents tasks` 查找待审批任务，用 `teamagents status TASK_ID` 阅读具体操作，再将该操作的 `approval_id` 传给 `teamagents approve APPROVAL_ID` 或 `teamagents deny APPROVAL_ID`。`--check` 指定的原样命令已在提交时获得授权。

## 工作区的访问范围

默认情况下，Shell 命令与验收检查运行在 `bwrap` 的 **direct workspace-write** 直接挂载模式：工作区直接读写绑定为 `/workspace`，同时隐藏 `.teamagents-auth` 并把顶层 `.git` 与 Git 管理目录以只读方式挂载，不再受暂存复制体积、条目数或目录深度上限限制。每条命令仍有独立的沙箱家目录和 `/tmp`，安装到这两个目录的依赖不会保留到下一条命令。

如需暂存写回或只读隔离，可在提交或启动时传入 `--staged` / `--sandbox staged`（对话 / TUI 中使用 `/sandbox staged` 或 `/staged`）切换为暂存副本模式，或使用 `--sandbox read-only`（`/sandbox read-only`）将 `/workspace` 以只读方式挂载在 `bwrap` 内并禁用 `edit_file`/`apply_patch` 文件修改工具。暂存支持目录和只有一个硬链接的普通文件，上限为 256 MiB、32,768 个条目、64 层目录（拒绝符号链接、多硬链接、特殊文件和嵌套挂载，并排除 Git 元数据和带 `CACHEDIR.TAG` 的 Cargo `target` 缓存），并在命令正常退出后校验宿主机冲突再逐项写回（即使退出码非零也会写回；中途崩溃可能导致部分写入）。

Shell 命令支持模型自选 `timeout_ms`（`100..=600_000` 毫秒，默认 60 秒；超时返回 `exit 124` 及部分输出），并对 stdout / stderr 采用各流 128 KiB 的首尾保留中间截断（前 64 KiB + 后 64 KiB），不再因输出过多而中止命令。`edit_file` 与 `apply_patch` 单文件上限均为 8 MiB；`read_file` 默认读取 8 MiB 分片，并支持 `line_numbers` 行号显示及通过 `offset`/`limit` 分页读取超出 8 MiB 的文件。

**请仔细选择工作区。** Shell 能对外联网，也能把工作区内的文件（包括你放进去的密钥）发送给外部服务。沙箱不挂载宿主机的家目录、凭据存储或守护进程套接字。回环地址指向沙箱自身，但局域网中的其他服务仍可能可达。

## 会话与运行时边界

`--workspace DIR` 选择工作区，默认使用当前目录。其规范路径决定会话位置 `${XDG_STATE_HOME:-$HOME/.local/state}/teamagents-swarm/sessions/<workspace-hash>`，也可以用 `--session DIR` 指定会话目录。重新连接时沿用同一对路径。会话目录必须由当前用户所有，不允许组或其他用户写入，也不能与工作区重叠。

| 运行时边界 | 配置 / 数值 | 适用范围 |
|---|---:|---|
| 调用次数 / 成员总数 / 并发数 | 无限制 | 取消模型调用次数上限、会话成员数上限、并发数上限与同成员串行化 |
| Handoff / 并发批 / 验收 / 格式重试 | 64 / 16 / 10 / 5 | 单次最多子阶段数 / 单轮并行工具调用数 / 最多验收检查数 / Worker 连续协议纠错数 |
| 文件读取（`read_file`）与编辑 / 补丁 | 8 MiB（支持分页） | `edit_file` 与 `apply_patch` 单文件上限；`read_file` 支持 `line_numbers` 与 `offset`/`limit` 分页 |
| Shell 超时与输出 / IPC 帧 | 60s（最长 600s）/ 128 KiB / 8 MiB | 可配 `timeout_ms` / stdout/stderr 各流中间截断上限 / 守护进程 IPC 帧上限 |
| 模型 HTTP / 请求响应 / SSE 流 | 600s / 8 MiB 请求，2 MiB 响应（Reporter 12 MiB）/ 32 MiB | HTTP 读取超时 / Worker 请求与响应及动作上限 / SSE 流上限 |
| 自动/手动上下文压缩 / 白板摘要 | 模型窗口 90%（>=6 条）/ 4 KiB | 软触发自动压缩与手动 `compact`（支持 `TEAMAGENTS_CONTEXT_WINDOW` 与 8 MiB 封顶，另含 413/溢出）/ 增量白板摘要上限 |
| Summarizer 输入 / TUI / MCP | 2 MiB / 8 MiB / 1000 / 30s | 汇总叶子输出 / 意图澄清 / TUI 事件与 `@` 文件索引上限 / MCP `256 KiB` 配置、`30s` RPC、`2 MiB` 响应 |

每个会话只有一个守护进程写 SQLite，使用 WAL 和 `synchronous=FULL`。已启动的工具如果没有持久化结果，其效果未知，守护进程不会自动重放。开始新任务前请检查工作区及可能发生的外部操作。取消任务不会撤销已有操作，也不能证明命令已经停止。模型调用次数和并发数都不等同于 token 费用上限。

开发时，`make build` 生成 `target/debug/teamagents`，`make fmt` 格式化代码，`make check` 检查格式、运行 Clippy 和测试。`make package` 在 `target/dist/` 生成当前主机的二进制压缩包及 `SHA256SUMS`。
