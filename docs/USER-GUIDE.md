# 用户指南：配置、权限、恢复与故障处理

适用版本：当前 Rust 实现（2026-09-19 核对）。验收覆盖与尚未满足的方案要求见
[验收对照表](ACCEPTANCE.md)。

## 1. 配置

首次安装运行 `teamagents init` 即可创建最小配置，重复运行保留已有文件；密钥仍从环境变量读取。
下载安装、升级及旧版兼容说明见 [安装指南](INSTALL.md)。

Worker 的 `instructions` 是成员专属指令。内置 Worker 会先收到固定的 TeamAgents 环境 system prompt，
再拼接这些指令、选中的 Skills、指令文件和计划；省略 `instructions` 也仍有环境说明。
任务描述、验收标准、消息和团队状态由运行时另行投递。Codex 执行成员则在创建/恢复线程时通过
`developerInstructions` 接收环境说明与成员指令，保留其原生 system prompt，通过自身输出汇报执行结果。

### 1.1 位置

| 内容 | 位置 |
|---|---|
| 用户配置（模型 profile、工具绑定、Skills 目录、指令文件） | `$XDG_CONFIG_HOME/teamagents/config.toml`（默认 `~/.config/teamagents/config.toml`） |
| 项目配置（同名条目用户定义优先，不能开启全自动或扩大预授权） | `<项目>/.teamagents/config.toml` |
| 会话状态（业务库、成员私有检查点、制品、成员工作目录） | `$XDG_STATE_HOME/teamagents/sessions/<session_id>/` |
| TUI 语言偏好 | `$XDG_STATE_HOME/teamagents/ui.json`（默认 `~/.local/state/teamagents/ui.json`） |
| TUI 输入历史 | `$XDG_STATE_HOME/teamagents/composer-history.json`（上限 500 条，跨会话与重启保留） |
| 团队定义导入 | 任意路径的 JSON/YAML，`teamagents validate` 校验、`--team` 用于新会话；暂无专用导出命令 |

### 1.2 模型 profile

```toml
[models.leader_main]
provider = "deepseek"          # 逻辑名，也用于 codex 成员的 provider 映射
protocol = "deepseek"          # responses | anthropic | openai | deepseek
model = "deepseek-flash"       # 首版默认模型
api_key_env = "DEEPSEEK_API_KEY"          # 只引用环境变量，密钥不进仓库
timeout = 120
max_retries = 2               # 示例显式覆盖；省略时默认 5
generation_options = { reasoning_effort = "max" }     # 默认档位；嫌慢改 high（实测 high≈10s / max≈240s）
context_window = 1000000   # 可选；填写后启用上下文自动压缩（见 §3.4），/status 也会显示占用比例
```

Codex 成员（`runtime_kind: codex`）走 `codex app-server`：给它所在的 model profile 加一行
`codex_profile = "deepseek"`，引擎就会把 `$CODEX_HOME/deepseek.config.toml` 里的设置
（`model_provider`/`model`/provider 的 `base_url`/`env_key` 等）展开成 `codex app-server -c ...`
覆盖，于是这个成员跑在 DeepSeek 上而不是你的 Codex 官方订阅。profile 文件不存在时该成员会明确报错。

推理档位规则：模型不支持 `xhigh` 时，配置里的 `xhigh` 会自动映射为 `max`
（DeepSeek 这类已知不支持的在构建模型时就映射；其他供应商在被拒绝后自动改判 `max` 重试一次）。

模型接入走 HTTP 协议直连，三种主流线上格式都已支持，`protocol` 选一种即可：

| `protocol` | 端点 | 典型用途 |
|---|---|---|
| `openai` | `POST {base_url}/chat/completions` | OpenAI 兼容服务：DeepSeek、GLM、Kimi、各类中转 |
| `deepseek` | 同上 | 同 chat/completions，另把 `xhigh` 档位映射为 `max` |
| `anthropic` | `POST {base_url}/v1/messages` | Anthropic 官方订阅与兼容网关（thinking/signature 随历史保留） |
| `responses` | `POST {base_url}/responses` | OpenAI 官方 Responses API 与 Codex 风格网关（工具按 `function_call`/`function_call_output` 往返） |

省略 `base_url` 时按 `provider`/`protocol` 取默认端点（`deepseek` → `https://api.deepseek.com/v1`，
`anthropic` → `https://api.anthropic.com`，其他 → `https://api.openai.com/v1`）。第三方服务要么与
这些端点同源，要么显式填 `base_url`；官方订阅只要把订阅支持的模型名写进 `model`、按上表选
`protocol`，即可直接用。

普通 Chat 成员默认请求模型 SSE；文本增量即时显示，工具参数完整接收后才执行。服务端不提供 SSE 时
兼容单个 JSON 响应。Anthropic 的 thinking/signature 块随历史保留；Responses 的推理增量只用于
内部，不进入候选回复。四种协议都有回归：`chat_e2e::responses_protocol_round_trips_a_tool_call`
（Responses 工具往返 + `instructions`/工具扁平化）、`stream::tests`（三种 SSE 解码）。

Codex 成员使用本机 `codex app-server`。当前引导会把所引用 profile 的 `model` 和 `provider`
映射给 Codex，不能假设总是继承本机默认模型；该 provider 必须是本机 Codex 可用的配置。
使用 `/model` 选择带 `base_url` 的 OpenAI 兼容 profile 时，会显式配置 Responses API 地址与
密钥环境变量。仅支持 Chat Completions 的端点不能直接用于 Codex。Codex 默认档位为 `xhigh`
（不支持时回退 `max`），会话 `/model` 的档位覆盖优先。

### 1.3 工具绑定（绑定即授权）

```toml
[tools.web]                    # 网页搜索（AnySearch HTTP；也可换成 Tavily 等 MCP 服务）
kind = "web_search"
provider = "anysearch"
url = "https://api.anysearch.com/v1/search"
api_key_env = "ANYSEARCH_API_KEY"

[tools.fetch]
kind = "web_fetch"

[tools.notes]                  # 本机 MCP 服务（stdio）
kind = "mcp"
mcp_server = "notes"
mcp_transport = "stdio"
command = "npx"
args = ["-y", "some-mcp-server"]
required = false               # 必需服务不可用会明确阻塞；可选服务失败只丢该能力

[tools.wiki]                   # 远程 MCP 服务（streamable HTTP，MCP 2025-06-18）
kind = "mcp"
mcp_transport = "http"
url = "https://mcp.example.com/wiki"
bearer_token_env_var = "WIKI_MCP_TOKEN"  # 密钥只从该环境变量读取，绝不写进配置
startup_timeout_s = 60         # initialize/tools/list 超时秒数（可省，默认 60）
tool_timeout_s = 120           # tools/call 超时秒数（可省，默认 120）
```

客户端向服务器声明 `roots` 能力：服务器发来 `roots/list` 时会得到该成员的工作目录
（`file://…`），其它服务器发起的能力（如 `sampling/createMessage`）按规范回 `-32601` 拒绝。
stdio 与 HTTP 两种传输都会应答。

HTTP 传输按 MCP streamable 规范实现：POST 的响应可以是单个 JSON 或 SSE；初始化后客户端会额外开一条
**GET SSE 推送流**（服务器通知写入 stderr；服务器发来的请求如 `sampling/createMessage` 会按规范收到
JSON-RPC 错误回复），会话结束（成员绑定关闭）时用 **DELETE** 终止服务端会话。
不支持推送/终止的服务器回 405 时按无推送处理，功能照常。

成员移除后其工具绑定会关闭，而不是一直保留到整个会话退出。stdio 服务在独立进程组内运行，
关闭或初始化失败会终止同组子进程并释放等待中的请求；HTTP 只发送一次 DELETE，关闭后拒绝新调用。
这不能撤销已发送的远程操作；在途 HTTP 请求仍受原超时限制。显式宿主模式的服务若主动脱离进程组，
其后台进程不受此清理保证覆盖。

旧式 `mcp_transport = "sse"` 已从 MCP 规范移除，绑定会直接报错并提示改用 `"http"`。

内置能力名 `files` / `shell` / `web` / `skills` 不需要同名配置条目，`files` 绑定同时包含
`view_image` 与 `edit_files`（一次提交多个文件的唯一匹配编辑：**全部校验通过才落盘**，
同一文件一次只允许一条编辑，返回各自 diff）（看图：png/jpeg/gif/webp，单张 ≤5 MiB）；模型是否真能"看见"取决于模型本身，
接口侧按协议自动转换：chat completions 把图片作为 user 消息的 `image_url`、Anthropic 放进
`tool_result` 的 image 块、Responses 放进 `function_call_output` 的 `input_image`。
但 `web` 仍需配置实际的
`kind = "web_search"` / `"web_fetch"` 服务，`skills` 仍需配置注册目录。
有显式网页绑定时按成员绑定顺序各选一个搜索/抓取服务；没有显式网页绑定时，`web` 才从配置中
按名称排序选择。要同时使用上例搜索与抓取，绑定 `[files, shell, web]`，或把搜索配置改名为
`search` 并绑定 `[files, shell, search, fetch]`。`[web, fetch]` 因存在显式 fetch 绑定，
只会启用抓取，仓库 `examples/team.yaml` 当前也有这一限制。
AnySearch 搜索需要密钥，原生 `web_fetch` 直接抓取目标 URL，无需 AnySearch 密钥。

### 1.4 Skills 与指令文件

```toml
# 顶层键：放在第一个 [models.*] / [tools.*] 表之前；目录必须存在
skills_paths = ["~/.agents/skills"]
# 可选：额外指令文件；只填写已存在的文件
# instruction_files = ["/绝对路径/extra-instructions.md"]
```

项目根及用户配置目录的 `AGENTS.md` 都会自动加载。
项目配置中的 `skills_paths` / `instruction_files` 与工具绑定一样，需要用户配置
`[permissions] trust_project_tools = true`；自动发现的项目 `AGENTS.md` 不受此开关控制。
Skills 不授予任何新权限。按 D-23，本项目的用户级注册根统一为 `~/.agents/skills`。

按 D-34，当前配套技能为：

- `K-Dense-AI/scientific-agent-skills` 科学技能集合：安装为多个独立技能目录，成员按具体名称
  （如 `scientific-writing`、`scanpy`）选用。
- `browser-use`：浏览器操作。
- `find-skills`：查找技能。

Skills 的加载与分发（对应方案 §12.1 的“发现 + 按需读取”）：

- **发现/按需读取**：成员在 `tool_bindings` 里绑定 `skills` 即获得 `skill` 工具
  （`action="search"` 按关键词检索名称+简介，`action="read"` 按名取全文）。注册根就是
  `skills_paths`，只读；项目 `.teamagents/skills` 在工作区内，用 files 工具即可读。
- **分发**：TeamSpec 或 topology patch（`add_agent`/`update_agent`）里的成员 `skills: [名称]`
  会把对应 SKILL.md 内容注入 Chat 成员系统提示词（每文件最多 8,000 字符，Skills 与指令合计
  每成员 32,000 字符；同名按用户级 → 项目级 → 成员级覆盖）。Leader 据此将选定技能分给特定成员。
- 默认新会话 Leader 只绑定 `files/shell/web`；需要技能检索时，在 TeamSpec 或 patch 中追加
  `skills` 绑定。上述注入与 `skill` 工具属于 ChatRunner，未接入 CodexRunner。

## 2. 权限

### 2.1 两种模式

- **approved_scope（默认）**：预授权范围内自动执行；范围外生成批准请求。
  初始预授权 = 当前工作目录读写、被绑定的工具（文件/搜索/MCP）、**无网络**的隔离 Shell。
- **full_auto（仅用户可开启）**：跳过逐次批准，仍保留团队通信 ACL、动作校验、记录与执行上限；
  不绕过操作系统与外部服务的限制。TUI 状态栏始终显示当前模式。

当前原生文件工具仍限制在成员工作目录、会话共享的 `/artifacts/` 和当前成员只读的 `/tool-output/`，Shell 仍使用 bubblewrap，
全自动不会扩大其挂载范围；`network=true` 的 Shell 可免批准联网。越界文件路径直接失败，
尚无通过批准扩展文件根的流程。Codex 后端则按权限模式映射自己的沙箱策略。

切换：TUI `Ctrl+F`，或 `teamagents --full-auto`，或用户配置
`[permissions] mode = "full_auto"`（非法取值会直接报错，doctor 可见）。项目配置**不能**开启全自动。
权限模式以会话行为准：审批门每次调用前读取会话的权限模式，切换立即生效、无需重开。

### 2.2 批准语义

- 批准绑定**具体操作与参数**；参数变化需要重新批准。
- 提供三种决定：本次批准（once）、会话内批准（session）、拒绝。
- **once 批准在执行后即消费**（置 EXPIRED），同一操作再次执行需重新批准；session 批准（以及尚未
  消费的 once 批准）只在策略修订（revision）不变时放行；EXPIRED 的记录按“需重新请求”处理。
- 等待批准只暂停相关操作，其他成员继续；`WAITING_APPROVAL` 不算回合结束。
- 恢复时重新核对参数、配置版本与权限，历史批准不会沿用失效范围。
- Codex 成员的批准等待有 600s 上限，超时把该批准置 EXPIRED（需重新批准）；其他成员不受影响。
- Codex 的会话内批准也绑定具体操作：相同命令/目录等可复用，不同操作重新请求；线上仅回复
  单次 `accept`，不使用 Codex 的宽范围 `acceptForSession`（D-31）。

### 2.3 隔离边界（诚实说明）

- Shell 走 bubblewrap：只挂载系统只读目录 + 授权工作目录，隔离 PID/网络/临时目录；
  网络默认关闭，需要联网的操作要批准。**要求** bwrap：缺失时命令直接失败
  （`IsolationUnavailable`），不会退化成不隔离执行；命令环境是白名单（不含模型密钥）。
- Shell 续用状态：同一个成员的 `cd` 与 `export` 会跨命令保留，状态存在
  `sessions/<id>/members/<成员>/shell/`，随会话保留、重开后仍生效；
  命令输出以 `[cwd: …]` 开头，目录取自 Shell 的实际位置。除成员私有 HOME 外，工作区挂载之外的临时文件只在
  **单次调用内**保留；使用 `/tmp` 验证副本时，应在同一条命令中完成复制、检查与结果输出，
  需要跨调用保留的文件应放在允许写入的工作区内。保存的目录消失时，当前命令跳过并返回
  `ShellStateUnavailable` / `(exit 1)`，下一次调用从工作区根目录开始，普通导出变量保留。
  被中断/超时的命令不会更新状态；状态快照用临时文件 + rename 写入。
- Shell 的默认 `HOME` 是成员自己的 `shell/home/`，用于 npm/pip 等工具的缓存与配置；
  不把项目目录当作 HOME，不导入宿主用户的 HOME。缓存随会话恢复，其他成员不共享。
  旧无版本 Shell 快照中 HOME 等于项目根时按旧默认值迁移；已有项目内 `.npm` 等文件原样保留，
  不自动移动或删除。新快照中显式 `export HOME=...` 的选择会继续保留。
  无持久状态的 Shell 与工作区模式 MCP 使用临时私有 HOME，退出后清除；MCP 的 host 模式沿用用户授权环境。
  缓存文件是普通副作用，中断不会回滚，当前随会话保留策略清理，尚无独立容量上限。
- `/artifacts/` 与 `/tool-output/` 是文件工具的虚拟路径，未挂载进 Shell。
  共享交付物用 `write_file` 写到 `/artifacts/`；自动长输出用 `read_file` 读取 `/tool-output/`。
- 构建工具链只读镜像：宿主 `$HOME` 在沙箱里不可见，因此 sandbox 会把 `RUSTUP_HOME`（默认
  `~/.rustup`）与 `CARGO_HOME` 的 `bin`/`registry`/`git` 子目录只读挂到沙箱内
  `/tmp/.teamagents-toolchain/`，并设置对应环境变量。成员因此可以真的 `cargo build/test`
  （含离线 registry 缓存）；`credentials.toml`/`config.toml` 不挂载，所以注册表令牌不会
  进入沙箱。只有 Rust 已按此镜像处理，其它语言的 HOME 级工具链（nvm/pyenv/uv/pnpm 等）仍不可见。
  系统安装的 Python/Node 可用；本地回归已覆盖 Python venv 续接、离线 wheel 构建/安装，
  以及 Node 本地依赖安装、离线 `npm ci`、测试、构建和打包。实际项目仍须具备所需解释器与依赖。
- 文件工具做符号链接与路径穿越防护，越界即拒绝。
- Shell、grep 等自动捕获的输出属于调用成员，保存到 `members/<成员>/tool-output/`；
  输出中的 `/tool-output/exec-*.log` 用 `read_file` 分页读回。其他成员和 Leader 即使知道同一引用也不能读取。
  需要交付时，成员应主动将可共享内容写入 `/artifacts/`，再经授权消息或共享空间告知接收者。
  `/artifacts/` 本身是会话共享目录，不能用于存放私有资料。
- `publish_shared.ref` 必须是非空字符串，`complete_task.result_refs` 必须是非空字符串组成的数组，
  也可省略引用或传空数组。私有上下文/线程 ID、成员历史、自动输出、运行数据库和配置文件不能作为附件发布；
  本机路径会检查符号链接、路径穿越、`file:` 编码和行号后缀。普通工作文件、共享制品和外部证据 URL 仍可引用。
  发布引用不会复制文件或授予接收者读取权限。回合正常结束时会复核承接者、任务状态和引用；
  已取消、已结清或已移交的任务保持当前结果，迟到申请只保留审计。
  引用失效且当前附属任务仍归原成员、尚未结清时，该任务进入 `BLOCKED` 并保留原因。
  此检查不代替成果内容及验收条件的验证。
- 团队动作的外层参数必须是 JSON 对象，错误类型和未知字段会被拒绝；消息、求助、共享正文和完成摘要
  不会把数字或布尔值转成文字。参数错误会明确回给成员，可修正后以新的调用 ID 重新提交。
  原调用 ID 会继续重放原回执，修改同一 ID 的参数会被拒绝。
- 任务、父任务、依赖、求助关联任务及回合 ID 必须来自当前会话。
  已完成、失败或取消的回合不能再提交新动作，原动作的相同请求仍可按持久回执重放。
- `publish_shared.supersedes` 可指向当前会话中该成员有权读取的旧条目；修订以新条目追加，原条目保留。
  `read_shared` 省略 `space_id` 时读取所有获准空间；省略 `after_sequence` 时分别沿用各空间的已读位置，
  合并后按序列分页。显式传 `after_sequence: 0` 可重读，`limit` 必须是正整数；
  空值、字符串数字、布尔值和小数都不能冒充这些分页整数。读取失败不会部分推进其他空间的游标。
  `list_shared` 返回实际条目数和最新序列，不改变已读位置。
- 共享工作目录不能包含或位于 TeamAgents 状态目录、用户配置目录及 `CODEX_HOME`；
  发生重叠时，会话/成员启动返回「工作目录与私有运行数据重叠」，不会将该目录交给模型执行。
  请选择独立项目子目录，或把 XDG 配置/状态目录移到项目外。符号链接按实际目录核对。
  管理的 isolated/worktree 根仍限定在成员自己的 `work/`，不包含其旁边的历史和 Shell 状态。
- 写入用"进程内互斥 + 跨进程文件锁 + SHA-256 版本校验"：锁文件放在 `sessions/<id>/locks/`，
  另一个 teamagents 进程正在写同一路径时会等它写完；等待超过 10 秒则以
  `another teamagents process is writing this file` 拒绝。文件系统不支持建议锁时退化为原来的
  进程内互斥。
- 已绑定的 MCP 工具由 ChatRunner 直接调用，不再逐次批准。stdio 服务默认在成员 workspace
  的 bubblewrap 中运行，使用白名单环境，网络默认关闭；需要网络时显式设置 `mcp_network`。
  只有显式配置 `mcp_execution = "host"` 才使用宿主权限。HTTP 服务仍由远端授权控制。
- “私有上下文隔离”是运行时投递与工具授权合约；显式宿主模式的 MCP 按当前用户权限运行，
  外部后端还受各自的执行策略约束。该合约不承诺对恶意同用户宿主进程的强保密隔离。
- 通道、观察范围或共享空间权限变更经拓扑 patch 生效后，尚未注入的旧投递会重新检查：
  无权内容失效并保留原因，观察范围缩小时按新范围裁剪。再次授权不会补发失效消息或扩大旧投递；
  已进入成员上下文的内容无法撤回。仅修改观察范围也会等待接收成员的安全执行边界。

### 2.4 事件钩子（可选）

```toml
[hooks]
notify = ["/home/you/bin/teamagents-notify.sh"]   # argv；事件名追加为最后一个参数
```

`[hooks]` 与 `[retention]` **只在用户配置里生效**：项目目录的 `.teamagents/config.toml` 里写这两段会被
忽略并打印一行提示——钩子会执行命令、保留策略会删数据，克隆下来的仓库不该有这种权力。

配置后，引擎在这些事件发生时把事件 JSON 写到钩子的 stdin：

`pre_tool`：**执行前的策略钩子**（原生工具，即文件/Shell/网页这些），同一份 JSON 走 stdin：

| 退出码 | 效果 |
|---|---|
| 0 | 放行 |
| 2 | **拒绝**，stderr 第一行作为原因回给模型（形如 `denied by pre_tool hook: …`） |
| 其它 / 启动失败 / 超过 10 秒 | 放行并打日志——写坏的钩子不该让团队停工 |

```toml
[hooks]
pre_tool = ["/home/you/bin/policy.sh"]   # 每次都阻塞等它退出（上限 10 秒）
```

| 事件 | 何时 | 载荷要点 |
|---|---|---|
| `tool_call` | 原生工具执行完（Chat 成员） | `agent_id`、`tool`、`ok`、`error`、`arguments`（≤500 字符） |
| `run_completed` / `run_failed` / `run_cancelled` / `run_paused` | 回合进入终态（或停在批准/等任务） | `run_id`、`agent_id`、`status`、`error`、`reply_text` |
| `team_action` | 任何团队动作被受理 | `kind`（assign_task/complete_task/signal_done/…）、`actor_id`、`ok`、`payload` |

回合通知在归档成功后发送；已结清结果的重试不重复通知，批准已处理并继续执行时不发送暂停通知。

行为边界：钩子是**你自己写的程序**，在主机上以你的权限运行（不进成员沙箱），所以不要在里面回显密钥；
每次事件单独起进程，10 秒未结束会被杀掉，失败只写 stderr，绝不影响回合。

## 3. 恢复

- 正常退出默认保存并暂停；异常退出后下次启动自动恢复：团队版本、待办任务、消息位置、
  成员私有线程与批准队列都会重新装载。Chat 对话以 `members/<成员>/chat_tree.json`
  的追加式节点树和 leaf 指针保存；旧 `chat_history.json` 会惰性迁移，仍保留线性兼容快照。
- 执行意图（`QUEUED` 回合）先持久化再执行，恢复后继续；ChatRunner 在模型响应、工具调用与
  结果边界保存回合检查点，恢复时沿用原工具调用 ID，通过核心回执去重。Codex 回合先核对外部历史。
- 执行中的 Chat 回合若缺少有效检查点，或外部工具已开始但没有落盘结果，进入
  `OUTCOME_UNKNOWN`，需核对已产生的结果再决定后续任务。
- 回合结果归档失败时，运行时保留原结果并重试核心事务，不重新执行成员，也不提前通知回合完成。
  Chat 已知模型/协议失败会先保存终态检查点；归档失败后重启可归档同一失败结果，不再次请求模型。
  数据库持续不可写时仍无法完成归档，需要排除存储故障；原副作用不会因此回滚。
- 已返回结果、等待读取或归档恢复的 Chat 回合会保留其可确认的成功/失败结果；
  迟到的取消请求不会把已有终态改写为取消。重启先核对并恢复私有历史提交日志，
  再归档已知结果，不为读取最终回复而重新排队或请求模型。
- 旧版留下的排队回合若已有检查点、外部回合 ID 或数据库开始事件，会先核对已有执行结果，再处理后续调度。
  重开时提交新输入或开启全自动也先做这项准备；准备受存储故障阻止时，本次新输入明确失败，
  排除故障后需重新提交。已有工作继续按原记录恢复，损坏检查点不会当作新任务重做。
  只有开始事件而缺少有效检查点的 Chat 回合会进入结果不明，不自动重做；开始事件不能还原丢失的结果。
- 回合开始前读取成员输入失败，会保留同一个排队回合等待恢复；成员已返回后读取状态失败，
  也会保留原结果等待归档。TUI 可读取状态时显示“等待存储恢复”和具体原因，
  同一错误不会反复刷屏，恢复后有提示。退出后的恢复仍依赖成员检查点或外部回合记录。
- 运行中已经受理的补充和成员消息会自动派送到接收成员；投影读取失败显示 `delivery` 阶段，
  故障解除后自动重试，无需为触发重试再次发送。消息仍在下一次模型调用前按当前权限注入，
  不会修改已经发出的模型请求。恢复时重新排队受阻则显示 `reconcile`，修复后继续核对原回合。
- 读取任务依赖、回合输入、事件、投递或批准时发现损坏的持久 JSON，会明确报错并保留原记录，
  不按空记录继续，也不把坏投递当作权限撤销而丢弃。批准过期失败会回滚同一次结算。
  启动预检读到坏业务记录时，`--team` 或全自动选项也不会绕过错误或先切换权限。
  先关闭会话、保留会话目录备份，再核对错误字段并从已确认的备份恢复；产品不自动修复坏数据库，
  启动预检也不是对全部历史的完整扫描。检查范围见[持久记录验证](../review/stored-integrity-2026-09-19.md)。
- Codex 重开时按保存的线程 ID 与回合 ID 核对历史；已完成的结果补齐任务结算，
  已提交的完成申请保持原样。不会把线程中后续回合的结果记到旧任务上。
  外部 ID 缺失、历史无法确认、提交后连接断开或 app-server 中途退出时保留结果不明状态。
  这类回合中未确认的旧输入会记录原因并停止自动投递，原事件仍保留，也不会被伪记为已消费；
  核对成果后可结清旧回合并明确下达后续工作。
- 原 `OUTCOME_UNKNOWN` 回合取得可确认结果后，可结算其自身仍归原成员的 `BLOCKED` 任务；
  已取消或已移交的任务保持当前状态。旧回合归档不会复活已移除成员，也不会将正在执行新回合的成员改为空闲。
- **四类崩溃窗口**都有处理：提交后尚未启动、模型已完成但结果未归档、团队动作已提交但回执未落、
  外部工具已执行但结果未知。最后一种进入 `OUTCOME_UNKNOWN`，**不会**被当作成功或自动重试。
- 取消执行中的回合需要等待停止确认；尚未开始且无外部回合 ID 的排队回合可直接取消，即使其成员输入视图暂时不可读。
  取消整回合会给该回合尚未消费的输入记录失效原因，阻止旧输入再次启动回合；
  取消单个任务只移除该任务的待投递就绪通知，同回合其他消息和任务继续保留。
  取消、批准失效与输入处置在同一事务提交，存储失败会拒绝本次操作并回滚，需排除故障后重新取消。
  已产生的文件、请求与制品保留；已有外部回合仍需核对停止状态，取消不代表副作用回滚。
- 成员身份是 **会话 ID + 成员 ID**，显示名称不是身份。恢复原会话继续使用原成员上下文；
  移除后同名重建必须使用新成员 ID，新会话即使用相同成员 ID 也不继承旧私有历史或模型覆盖。
  移除在安全边界生效后后台关闭运行器（暂停态也会清理），会话退出会等待该清理；原工作目录、
  已记录的对话/结果和审计仍保留。它们不会因此自动注入 Leader；用户可用下述只读浏览器查看本地已保存记录。

### 3.1 会话记录在哪里、怎么管

```
$XDG_STATE_HOME/teamagents/sessions/<会话 id>/     # 默认 ~/.local/state/teamagents/sessions/
├── team.db            业务事实：动作回执、事件流、任务、回合、投递、批准、共享条目、拓扑补丁
├── artifacts/         主动交付的会话共享制品（/artifacts/ 前缀；文件工具可读写，ls/glob 与隔离 shell 看不到）
├── members/<成员>/tool-output/  自动捕获的私有工具输出；/tool-output/exec-*.log 仅当前成员可读
├── members/<成员>/work/  隔离/worktree 成员的专属工作目录（文件工具的执行根）
├── members/<成员>/shell/  Shell 目录/导出快照；home/ 保存成员自己的工具配置与缓存
├── members/<成员>/worktree.json  新建 worktree 的基线提交与归属分支；旧会话没有此文件也能恢复
├── members/<成员>/review-root.json  实际工作目录记录（审查专用，不注入成员上下文）
├── reviews/<根目录哈希>/  首次观察的文件清单和文本快照；同会话共享工作根只存一份
├── members/<成员>/chat_tree.json     对话树、当前 leaf、回退代次（含保留的旧分支）
├── members/<成员>/chat_history.json  当前线性历史快照及旧版迁移入口
├── members/<成员>/turns/<回合>.json  回合检查点（原工具调用 ID、执行进度、结果与已注入投递）
├── profiles.json      自动创建的会话级模型 profile
├── model_overrides.json  /model 设置的成员覆盖
└── session.lock       执行所有权（flock；同一会话同时只允许一个运行实例，kill -9 自动回收）
```

升级前保存在 `artifacts/exec-*.log` 的自动输出没有可靠成员归属。原文件保留供用户检查，
成员文件工具不再读写这类旧文件或指向它们的符号链接，共享附件和任务成果引用也会拒绝它们；
不会猜测归属或自动迁移。
当前成员仍可用 `read_history` 读回自己历史中已经保存的工具结果，但它不能恢复当时没有写入历史的输出尾部。

### 3.1.1 成员记录（用户只读浏览器）

输入框执行 `/history` 可从成员列表开始浏览；在团队或日志页签选中成员后按 `h`，可直接打开该成员的记录。
当前及已移除成员都保留在列表中。按 `Enter` 依次进入成员、来源和详情；`n`/`p` 翻页，`r` 刷新，`i`
查看范围说明，`Esc` 返回。正文过长时可用方向键、PgUp/PgDn 或 `Ctrl+U/Ctrl+D` 滚动。

可浏览的本地来源有：

- Codex 原生对话与工具记录：有持久线程 ID 的成员会显示此来源，可查看后端保存的消息、工具调用及结果；
- `chat_tree.json`：完整的已保存对话树，包括当前 leaf、父节点、回退分支和压缩前仍保留的节点；
- `chat_history.json`：线性兼容快照；旧会话没有树文件时只在内存中转换，不触发迁移；
- `turn_runs` 与 `members/<成员>/turns/<回合>.json`：回合元数据和已保存检查点；
- `team.db` 中与成员明确相关的事件：按行为者、成员字段和历史受众筛选，不把普通文本碰巧出现成员 ID 的事件算进去。

这是**面向用户的只读记录查看**：读取使用独立只读数据库连接和受限文件打开。选择 Codex 原生来源时会按需启动
独立 App Server 读取已保存线程，不恢复成员线程、不启动模型回合。私有记录不会注入 Leader，
读取不确认投递、不执行记录中的工具、不改变任务状态。文件、单条事件或后端单条响应超过 32 MiB、
损坏、非普通文件或符号链接会明确报错；页面携带版本与游标，内容变化时按 `r` 刷新后重新选择。

Codex 原生分页接口不可用时，会尝试读取后端提供的本地记录；文件必须位于配置的 `CODEX_HOME`
（缺省为 `$HOME/.codex`）的 `sessions/` 或 `archived_sessions/`，且线程身份匹配。
声明使用其他存储模式的文件不能作为旧格式回退，此时会提示更换支持该线程原生分页的后端。
旧格式条目可能没有回合 ID，列表用 `—` 表示没有已知关联，详情保留实际原文。
没有线程引用时显示说明；接口或文件不可读时显示错误，可修复后刷新。

TeamAgents 只展示后端提供或已经落盘的内容，不保存 Codex 历史副本。未落盘的流式文本、丢失记录以及后端没有提供或
加密保存的内部推理不能还原。旧文件每页重读且有大小上限；大历史与跨文件原子快照不在当前保证内。

```bash
teamagents sessions            # 列出会话：状态、目标、事件/任务数、占用空间、路径
teamagents --resume <会话 id>   # 恢复该会话（团队版本、待办、消息位置、成员线程、批准队列）
```

TUI 里同一件事在「会话」面板完成（`Tab` 把焦点移入管理面板，或 `Ctrl+T` 切页签后点击面板）：
`s` 或 `Enter` 切换、`n` 在当前目录新建会话、`a` 归档、`d` 删除（连按两次确认）。
面板动作只认无修饰的字母键：`Ctrl+A/S/D/C` 不会误触发归档/删除/批准/取消（`Ctrl+D`/`Ctrl+U` 是滚动）。
**归档/删除的是当前会话时会直接退出 TUI**；其他会话操作后留在原地并刷新列表。
运行中的会话（其他进程持有文件锁）不允许切换/归档/删除，会明确告知
（提示 `session <id> is already running (pid <n>)`）。

- **默认会话 id** 由工作目录派生（`proj_<12位哈希>`）；再次打开同一目录会复用默认会话。
  `--resume` 指向已有 id 时恢复它，指向未使用 id 时新建；不会自动复制其他会话上下文。
  `--team` 不强制新建或覆盖已有团队；要用另一个 TeamSpec 开新会话，请同时指定未使用的会话 id。
  已有团队配置损坏或违反结构约束时，恢复会报错并保留原记录；`--team` 也不会覆盖它。
  只有首次创建失败、尚未保存任何配置修订的会话，可以用修正后的 `--team` 重试。
- **归档**：移动到 `sessions/archived/<id>/`，保留数据库原状态。
  成员 worktree 的 Git 注册同步修复；修复失败时尝试移回并报告路径，不删除成果。
  若同名归档已经存在则拒绝覆盖，当前会话和旧归档都保留。
  TUI 的已归档行只展示，不支持切换、再次归档或删除，也没有取消归档命令。
- **删除**：使用会话面板 `d`，后端持有会话锁，先核对全部成员 worktree，
  再清理目录。未提交文件、被 Git 忽略的文件（包括构建目录）、冲突、未合并提交
  （包括 detached HEAD）、Git 锁或损坏元数据都会阻止清理，请先核对并保留所需成果。
  不删除成员切换到的用户分支；旧会话缺少分支归属记录时保留分支。
  这些检查不是跨目录原子事务，外部 Git/编辑器的并发修改和中途磁盘故障仍需人工核对。
  实现见 `engine/src/sessions.rs` 与 `engine/src/workspace.rs`。
- **导出/排查**：`team.db` 是普通 SQLite，可直接查，例如查看最近事件：
  ```bash
  sqlite3 ~/.local/state/teamagents/sessions/<id>/team.db \
    "select sequence,kind,actor_id,substr(payload_json,1,120) from events order by sequence desc limit 20;"
  ```
- **不记录什么**：模型密钥（只引用环境变量）、Codex 自身的会话历史（在 `~/.codex`，我们只存线程引用）、TUI 的界面状态（展开/光标等）。
- **并发保护**：会话文件锁 + SQLite 单写入口；第二个进程会明确报 “session is already running in another process”。

### 3.2 对话回退与分叉

TUI `/rewind` 列出当前分支的用户输入节点，`/rewind <序号>` 回到所选节点
（**包含该条输入**）；`/rewind 0` 清空当前对话路径。旧分支仍在树中，可用已知节点 ID
直接回退，但列表不展示全部分支。回退只改 Leader 的模型记忆，不撤销任务、事件或文件，
也不会清空已显示的聊天日志；Leader 有 QUEUED/RUNNING 回合时拒绝回退。

`/fork` 复制当前 TeamSpec、Leader 对话树、会话 `profiles.json` 与 `/model` 覆盖到新会话。
团队事实与其他成员历史重新开始；任意成员有 QUEUED/RUNNING 回合时拒绝分叉。
分叉打开失败时保留源会话。
`--plain` 使用 `rewind <节点 ID>`（无斜杠），不支持 fork；`model <成员>` 恢复默认模型。

`--plain` 只有收到成功回执才显示“输入已接收”；“输入被拒绝”会带出原因，不启动该次请求。
输入受理后若发生存储故障，会提示“等待存储恢复”或“读取状态失败”并返回命令输入；
原工作由后台重试，修复后无需重复提交消息。`status` 显示用量、新增事件与当前存储等待原因，
可查看原回复是否已归档。正常等待超过十分钟也会明确提示超时，后台仍继续处理。

若历史无法读取或结构损坏，恢复会在调用模型前停止并报告文件路径；
`/rewind`（包括 `/rewind 0`）会返回错误，不会用空历史覆盖原文件。
先关闭会话并备份其目录，再从已核实的备份恢复对应历史文件；不要靠删除记录绕过错误，
以免丢失任务上下文或重复已有副作用。没有可靠备份时保留故障会话，使用新会话前先核对工作成果。
历史恢复失败留下的结果不明回合仍按既有流程明确结清，不会自动重放。

### 3.3 磁盘占用与保留策略

- **自动输出**：Shell 等输出落 `members/<成员>/tool-output/exec-*.log`；超 200 KB 时返回私有读回引用，
  单文件最多 64 MiB，超出尾部丢弃并提示。每个成员的输出目录超过 512 MiB 时，新建输出文件会先按时间删
  最旧的 `exec-*.log`（自动，不需要配置）；被删的引用再读会报不存在。主动共享制品不按这项规则删除。
- **归档会话**：`teamagents sessions prune --days 30 [--dry-run]` 删除"最后更新超过 30 天"的
  归档会话；想让它自动发生就在用户配置里写 `[retention] archived_days = 30`（打开会话时清理，
  默认不写=不删）。两种方式都走同一套保护：运行中的会话、带未合并 worktree 成果的会话会跳过
  并报告原因。
- **会话数据库**：`teamagents sessions prune --history-days 30 [--dry-run]` 清理"已受理超过 30 天"的
  投递记录与对应事件，**未受理的投递和它需要的事件一定保留**；
  未结清回合（包含 `OUTCOME_UNKNOWN`）的开始事件也保留。读取或删除失败会回滚本次数据库清理，
  不留下只删除投递或部分事件的结果；实际删除后尝试 VACUUM。
  想自动发生就写 `[retention] history_days = 30`（打开会话时清理该会话，需要对上一次会话才有意义）。
  年龄按天算，默认不写=不清理——事件流也是 TUI 日志页签与审计的原料。
- **自动清理时保留**：回合检查点、对话树。它们分别是崩溃恢复、`/rewind` 与审计的
  依据；手动删除检查点会丢失可核对的结果。开始事件仍在时会停止自动重做并报告结果不明，
  所有执行依据同时丢失时无法保证识别旧工作。详见[恢复证据记录](../review/recovery-evidence-2026-09-19.md)。

### 3.4 上下文自动压缩（chat 运行时，D-28）

成员的模型上下文接近上限时自动压缩，无需任何命令；只对 chat 运行时生效
（codex 成员由 Codex 侧自行处理）。三层按成本递增：

1. **发送时限流**：工具结果字符串超过 50,000 字符时只截减**发给模型的那一份**（保留头尾各
   最多 25,000 字符、中间省略，并附 `read_history tool_call_id=…` 提示）。检查点与会话历史树
   保留完整原文，中间被省略的部分可用 `read_history` 按 tool_call_id 分页取回。
2. **视图遮蔽**：旧工具输出按由近到远的字节预算保留，超出部分替换为带读回指针的占位符。
   预算为 `context_window / 4` 字节，最低 16,000、最高 256,000；未配置窗口时使用 16,000。
   例如原生 1M 配置保留最近 250,000 字节。主成员和私有子代理分别处理自己的历史，
   最新一批未答工具结果不占这个预算；检查点与历史原文保留。
3. **阈值摘要**：`context_window` 已配置时，按最近一次 prompt 用量与下一请求估算值的较大者
   判断；超过自动阈值（不高于窗口的 90%，并预留输出空间）时，额外调一次
   模型把对话压成结构化摘要（目标/进展/文件/错误/任务/下一步）。**被压缩的原文不丢失**：
   仍在历史树中，`/rewind` 回退到压缩前节点即可再见全文；成员也可用 `read_history`
   工具按 tool_call_id 分页取回历史里保存的完整工具结果。压缩连续失败 3 次后
   当前 runner 自动停试，重建 runner/重开会话会重置（超限的 API 报错会原样暴露）。

`/status` 的用量账本写入成员目录 `usage.json`，重建 runner/重开会话仍保留；服务端未返回 usage 时计入未知调用，不猜测 token 或费用。

## 4. 非交互执行与故障处理

脚本和 CI 可使用 `teamagents exec --json PROMPT`。stdout 只输出 schema version 1 的 JSONL
（`session`、`tool`、`event`、`result`），诊断写入 stderr；`--check COMMAND` 可重复指定验收命令，命令在
隔离 Shell 中按顺序执行。退出码为：0 完成，1 失败或未完成，3 需要批准，124 超时。
非交互方式没有人能回应批准，因此回合停在待批准时立即返回 3（不等超时）：需要批准的工具
要么改用 `--full-auto`，要么先在 TUI 里批准再用 `--resume` 继续。
使用 `PROMPT` 为 `-` 时从 stdin 读取；`--resume ID` 可继续同一会话。

早先回合失败后若已恢复、目标已提交完成且全部 `--check` 通过，最终返回 0；历史失败记录仍保留。
超时、待批准、尚未确认的执行结果或验收失败不会因此被判为成功。
本次输入被拒绝或运行时存储读写失败时，结果包含 `runtime_errors`，跳过本次 `--check`，
返回 1；已经发生的超时仍返回 124。旧目标完成记录不会使未受理的新输入返回成功。
排除故障后可恢复会话；若错误阶段是 `input`，本次输入尚未受理，需要重新提交。

每行内容：

| type | 字段 | 说明 |
|---|---|---|
| `session` | `session_id` | 会话已打开，后续行都属于它 |
| `tool` | `run_id`、`agent_id`、`tool`、`call_id`、`ok`、`error`、`arguments` | 每次工具调用的实时记录；`arguments` 是最长 500 字符的摘要（写大文件时会截断），据此可审计"改了哪个文件、跑了哪条命令" |
| `event` | `event`（核心事件：`run_started`、`goal_done`、`leader_reply`、`approval_requested` …） | 团队事务事件流水 |
| `result` | `status`、`exit_code`、`duration_ms`、`usage`、`runtime_errors`、`verification` | 最后一行；`usage` 是各成员的真实 token 账本，`runtime_errors` 给出错误阶段、回合/成员及原因，`verification` 是 `--check` 命令的输出与退出码 |

| 现象 | 处理 |
|---|---|
| 界面提示“成员 leader 的模型 profile 'leader_main' 未配置” | 首次使用运行 `teamagents init`；若已有配置，按界面显示的路径补齐 `[models.leader_main]` 后重开会话（`init` 不覆盖已有文件） |
| 输入后长时间没有回应 | 先看对话视图：回合失败会以 `✗ …回合失败：<原因>` 显示；若没有该行且状态栏“活动回合 ≥1”，说明模型正在生成（xhigh 思考可能较慢）。最常见原因是模型 profile 未配置或密钥环境变量缺失（`doctor` 可确认） |
| 报缺少某环境变量 | profile 的 `api_key_env` 指向的变量未导出；导出后重跑 |
| 界面显示“等待存储恢复” | 查看同一条错误中的阶段和原因，排查会话目录、磁盘与数据库；故障解除后运行时重试。持久记录损坏时先关闭会话并备份，再按恢复章节处理；产品不会自动修复坏记录 |
| 命令因“refusing to run without isolation”失败 | 安装 bubblewrap；不要以降低隔离来绕过 |
| Codex 成员卡在“Reconnecting” | codex 的 provider 凭据不可达：检查 `~/.codex/config.toml` 的默认 provider 与密钥，或给成员配置走环境变量密钥的 profile |
| 回合因 `LIMIT_REACHED` 停止 | 达到目标回合数或模型请求步数上限（`limits.max_model_steps_per_turn` 真实约束模型请求数）；该回合记为 FAILED 并发 `limit_reached` 事件，调整 `limits` 后可继续 |
| 成员回合活动超时 | 超过 `limits.turn_active_timeout_s`（默认 1200s）会中断成员回合（不再继续执行）；回合记为 FAILED，按需重派任务 |
| 回合中断后"结果不明"（`OUTCOME_UNKNOWN`） | 回合在命令中途被中断，副作用无法确证，会一直挡住 `signal_done`。Leader 用 `cancel_run <run_id>` **明确结清**它（回执 `status=acknowledged`，事件里记 `acknowledged_outcome_unknown`）——这是人工确认"副作用我已接受、不再重试"；`signal_done` 的拒绝回执会直接把该 run id 与提示带出来 |
| 任务 `BLOCKED`（成员回合被中断） | 承接者已无法用 `complete_task` 结清（回执会说明）；**Leader 用 `cancel_task` 结清后重新派一次**（新任务，不复用旧 id），用户也可在任务面板按 `c`。团队不需要等人介入才能继续 |
| 任务长期 `BLOCKED` | 依赖失败或成员回合未提交完成申请等；Leader 可用 `cancel_task`，用户可在任务面板按 `c`。结清旧任务后按需创建新任务，不能用 `complete_task` 完成 BLOCKED 任务 |
| 需要查看发生了什么 | TUI 日志面板 / `sessions/<id>/team.db` 的 events 表 / `run_progress` 事件 |
| 隔离或协议自检 | `teamagents doctor`（依赖、配置、bubblewrap、codex、状态目录） |
| 提示“找不到 teamagents-tui” | 先 `cd tui && cargo build`；或用 `TEAMAGENTS_TUI=/路径/teamagents-tui` 指定 |
| 成员命令报 `IsolationUnavailable` | 未安装 bubblewrap；装上再试（不要用降低隔离的方式绕过） |
| `--team` 报 `bad spec` | TeamSpec 需为 JSON 或 YAML；`examples/team.yaml` 可直接使用 |

## 5. 团队定义（TeamSpec）要点

- 字段：`leader_id`、`agents[]`（id/name/role/runtime_kind/instructions/model_profile/
  tool_bindings/skills/workspace_policy）、`channels[]`、`observers[]`、`shared_spaces[]`、`limits`。
- `runtime_kind: deepagents` 是保留的兼容字面量，实际执行后端是 Rust `ChatRunner`，
  不依赖 Python/Deep Agents。
- 团队必须恰有一个 `role: leader` 成员，且由 `leader_id` 指向；该成员的
  `runtime_kind` 必须为 `deepagents`。其他成员可以使用 Codex 或自定义角色，
  但不能再声明为 Leader；导入、动态变更和恢复均检查此约束。
- `teamagents validate` 离线检查 TeamSpec 结构、成员/通道/观察/共享引用及正数上限，
  按 TeamSpec 所在目录加载用户配置与受信任项目配置。任务依赖环、Codex 委派者限制在提交任务时检查；
  校验通过不代表模型端点或工具服务可用。
- 运行中 Leader 用 `apply_topology_patch` 的 `add_agent` 创建成员时，`model_profile` 可省略：
  系统会为该成员自动创建**同名会话级 profile**（复制 Leader 当前生效的模型配置，含 /model 覆盖后的
  模型与档位）；填一个未配置的名字则视为模型 ID（沿用 Leader 的连接）；填已有 profile 名则直接复用。
  自动创建的 profile 存 `sessions/<id>/profiles.json`，随会话持久、仅本会话可见，重开后仍生效；
  之后可用 `/model` 单独调整该成员（D-30）。
- 普通成员可用 `propose_team_change` 提议，Leader 用 `patch_id` 批准后才建立成员；批准时也会
  补齐上述模型配置和下述工具、通道默认值。已有提案的 `operations` 省略或为 `[]` 时沿用原操作，
  非空数组则替换原操作。拒绝时必须给出 `patch_id` 和布尔值 `reject: true`，不能用字符串代替；
  内联变更必须携带当前整数 `base_revision`。错误类型、显式 null 和未知外层字段会被拒绝。
- 同一处 `add_agent` 还有两个默认行为（D-33）：省略 `tool_bindings` 时新成员**继承 Leader 的绑定**
  （显式写 `[]` 表示"只要团队工具"），并自动补上 `leader→成员` 与 `成员→leader` 两条 message 通道，
  所以 Leader 与成员一开始就能互相说话。成员之间不允许直接建通道：跨成员协作一律走共享空间，
  这样 Leader 与审计日志都能看到往来。
- 工作目录策略：`shared`（同一目录）、`isolated`（成员目录 + 明确输入/制品引用）、
  `git_worktree`（从明确提交建会话独立的成员分支与 worktree；**首次创建**时原目录脏则
  退回 shared 并说明原因；恢复时仍复用已有 worktree，不因主目录的新改动切换工作根）。
  worktree 与项目归属不符、Git 元数据损坏时明确报错，不静默回退；手动移动会话目录后，
  若 worktree 自身仍可读取，恢复会修复 Git 注册。未合并成果拒绝清理，删除会话同样受保护。
  三者均已实现（见 `docs/DECISIONS.md` D-19）。
- 实际默认 limits：工作成员并发 8（Leader 另有额度）、成员总数 20、单目标回合 1000、
  单回合模型步骤 200、活动超时 1200s、取消确认超时 60s（`core/src/models.rs::Limits`）。

## TUI（布局与交互）

> 按 D-20 设计：固定分区、滚动、胶囊状态、`/settings` 浮层，以终端习惯为准。

- **固定上下分区**：第 1 行是状态行（`TeamAgents「应用名」+ 会话 id + ⚠ 待批准 + ▸ 未完成`），
  上方是管理面板框（占约 2/5 高度、整宽），下方是聊天区（活动行、对话、流式预览、输入框），
  最后一行是页脚键位。分区与宽度无关，按键位置恒定。
- **管理面板**：六个页签（团队/任务/共享空间/批准/会话/日志），待办类页签带计数
  （如 `Tasks 2`、`Approvals 1`），选中页签用 `▍` 标出；面板底部一行是该面板的按键说明（英文）。
  表格随选中行滚动，列在窄终端下按优先级收起；空列表显示"— No tasks yet —"之类的提示。
- **团队页签的"最近活动"列**：显示每个成员最后一次工具调用与距今时间（失败带 `✗`），
  数据来自与日志页签同一条 `push:"tool"` 通道。
- **计划清单（`p`）**：在团队页签选中成员按 `p`，弹层显示它的完整计划（`[x]/[~]/[ ]` 三态），
  `Esc` 关闭、`Ctrl+U/Ctrl+D` 与 `↑/↓` 滚动；状态条只显示进度与"当前进行中"的那一项。
- **计划状态条**：成员用 `update_plan` 记录自己的待办（`[{text, status: pending|in_progress|done}]`）后，
  面板框下方会出现一行独立的状态组件：`计划 1/3 · leader  修 mul`（进度 + 成员 + 当前进行中的项）。
  计划是成员的"工作记忆"（存 `members/<id>/plan.json`，每轮随系统提示回灌给模型，不占团队任务语义）；
  选中哪个成员（团队/日志页签高亮，否则当前有回合的成员，再否则 Leader）就显示谁的计划。
- **结果不明的回合（`c` 结清）**：回合在命令中途被中断时，团队页签该成员的"状态"列显示
  `结果不明（c 结清）`（英文 `unknown (c to ack)`）；选中它按 `c` 即提交 `cancel_run` 结清
  （回执 `acknowledged`，事件带 `acknowledged_outcome_unknown`）。不结清会一直挡住 `signal_done`。
  `exec --json` 的结果行也带 `outcome_unknown: [run_id…]`，CI 可据此提示人工确认。
- **工作区审查（`/review`、`v`）**：`/review` 查看 Leader 的实际工作根；团队页签选中成员按 `v`，
  日志页签按 `v` 查看当前筛选成员（未筛选时查看 Leader）。文件清单来自磁盘与持久快照的比较，
  不依赖工具回执，包含文件工具、Shell、外部执行后端和用户留下的多批次变更，即使期间 Git 提交也保留。
  **共享根包含所有参与者的改动，不能归因某个成员**；isolated/worktree 查看各自实际目录。
  - `↑↓` 选文件，`Enter` 看 diff（绿色 `+`、红色 `-`）；`↑↓`、`Ctrl+U/Ctrl+D`、PgUp/PgDn 滚动当前页，
    `n/p` 切换 120 行分页，`←→` 横移。`i` 查看完整范围、警告、基线时间、文件大小/权限/哈希，再按 `i` 返回。
    `r` 从第一页刷新；分页期间检测到变化会拒绝混用新旧结果，需刷新重看。
    `Esc` 依次返回列表/关闭，`Ctrl+G` 切批准，`Ctrl+Q` 退出；读取在后台，不挡住这些控制操作。
  - 基线为本版本**首次使用该工作根时观察到的实际字节**，包括原有未提交输入，而不是 HEAD 或旧会话的历史起点。
    同一根的恢复会话沿用基线；旧会话升级后才建立第一份快照，不能恢复升级前的编辑历史。
    目录路径变更会建立另一份基线，归档只保留记录，不提供归档审查浏览器。
    初次捕获在工作根交给执行器之前同步完成，较大的目录会增加启动/新成员准备耗时；之后的审查读取在后台。
  - Git 范围为已跟踪文件与项目规则下未忽略的文件；全局 Git 配置不加载。非 Git 范围排除
    `.git/node_modules/target/.venv/venv/__pycache__` 目录。Git 元数据、会话私有状态不纳入；
    符号链接比较链接文本、不跟随目标；二进制仅比较哈希、大小和权限；重命名呈现删除与新增。
  - 限制：枚举至多 5000 项（非 Git 包含目录项）、单文件 2 MiB、每根快照内容预算 64 MiB；枚举与读取阶段
    分别检查 10 秒预算（不是对阻塞文件系统的硬超时）。Git 输出最多 2 MiB，单文件 diff 最多 20000 行、每行
    4000 字符；超限、特殊文件、读取异常均标不完整/未审查。不要把空列表、无文本差异或绿色 `+` 当成测试通过。
  - 这是只读核对，不是撤销、回滚或原子文件系统快照；并发外部写入仍应暂停后重新审查。文本基线以明文存在
    本机会话私有目录（0700/0600），可能包含任务输入中的敏感内容，不自动发送模型；每个新根单独占预算，
    会话总量没有独立快照配额，随会话归档/删除管理。首次捕获失败会明确缺少基线，不用修改后的内容悄悄重建。
- **日志页签**：除核心事件外，还实时显示每个成员实际调用的工具
  （`· edit_file alpha-fixer {"path":"alpha/alpha.py"}`，失败为 `✗`，参数截断到 120 字符）；
  按成员过滤时只显示该成员的工具行。日志在内存里只保留最新 2000 行（面板是环形缓冲）。选中成员按 `h` 可打开
  该成员的持久记录浏览器；它与日志筛选不同，不把未落盘的实时行当作完整历史。
- **活动行**：正在执行的成员以胶囊显示（`⠸ leader 00:12`），右侧灰字显示"最近 activity"；
  无执行时显示 `○ Ready`。
- **对话区**：底部对齐、可滚动（PgUp/PgDn、Ctrl+U/Ctrl+D、滚轮、Ctrl+Home/End），
  右侧细滚动条；上翻时右下角提示"已上翻 N 行 · Ctrl+End 回到底部"。Leader 流式回复带强调色竖条，
  回合结束后归档为正式消息；新提交会把视图拉回最新。
- **输入框**：圆角框、聚焦时边框变强调色；空输入显示占位提示；上边框右侧显示 Leader 状态与模型，
  下边框显示按键提示（按可用宽度自适应）。支持 `/settings` 命令。
- **斜杠命令**：输入框里以 `/` 开头即弹出候选菜单（可按前缀过滤，如 `/s`），
  `↑↓` 选择、`Tab` 补全、`Enter` 执行、`Esc` 关闭菜单。到最后一项再按 ↓ 保持选中；
  矮屏会随选中项滚动。内置：
  `/help`（键位与命令说明写入对话）、`/quit`（退出）、`/settings`（打开设置浮层）、
  `/status`（各成员 token 用量与上下文窗口占用）、`/review`（审查 Leader 工作区）、`/history`（只读浏览成员本地记录）、
  `/model`（打开模型选择器，
  依次选择 Leader 或团队成员、供应商、模型、思考强度；输入或粘贴文字搜索，Enter 确认，Esc 返回；
  候选合并 config.toml 中的 models 与供应商在线目录；进入供应商页后选择供应商，后台自动获取其模型，
  获取期间仍可选择配置中的模型，失败时显示原因并保留配置候选；在线发现的模型标“在线”，
  同名模型以括号内的 profile 名区分，供应商页可恢复默认；
  `/model <成员> <模型> [档位]` 仍可手输模型名，`/model <成员> clear` 恢复 TeamSpec 默认；
  覆盖只在当前会话生效，不改 TeamSpec）、`/rewind`（列出可回退点，`/rewind <序号>` 回到该节点并保留该条输入，
  `/rewind 0` 清空当前对话路径；具体节点与分支语义见 §3.2）、
  `/fork`（从当前 Leader 对话分叉为新会话，复制范围与限制见 §3.2）。
- **模型切换**：选择配置会一起切换 API 地址、认证环境变量、协议和上下文窗口，从下一回合生效，
  当前回合继续运行并可正常取消。覆盖随会话保存（`sessions/<id>/model_overrides.json`），
  重开同一会话仍然生效；`/model <成员> clear` 或选择器中的恢复默认会移除保存的覆盖。
  若重开时对应成员/profile 已不存在或不再合法，该条覆盖被静默丢弃。
  Chat 成员支持配置中的 Chat Completions、Responses、DeepSeek、Anthropic 模型；Codex 成员
  只显示 Responses 与历史 OpenAI 兼容配置，端点必须支持 Responses API。
  模型不支持所选档位时会显示服务端错误。
- **添加自定义供应商**：输入 `/model add`，或在 `/model` 选择成员后进入供应商页，
  选择「添加自定义供应商」。依次填写名称、API 格式（左右键选择）、基础地址、模型 ID、
  密钥环境变量名及原生上下文长度；Enter 下一项，↑ 或 Esc 返回修改，最后 Enter 保存。
  供应商只需支持以下一种 API，名称同时作为新 profile 的 ID，不能与已有供应商/profile 重名。

  | API 格式 | 基础地址示例 | 实际调用路径 |
  |---|---|---|
  | `responses` | `https://example.com/v1` | `/v1/responses` |
  | `anthropic` | `https://example.com` 或 `https://example.com/v1` | `/v1/messages` |
  | `chat/completions` | `https://example.com/v1` | `/v1/chat/completions` |

  基础地址不要包含具体调用路径、密钥或查询参数。模型 ID 可直接手填，供应商无需提供
  `/models`；保存后从配置候选选择即可。密钥栏只填环境变量名（如 `MY_API_KEY`），
  在启动 TeamAgents 的终端预先设置该变量；无需认证的本地服务可留空。
  上下文长度按模型原生窗口填写，未知时留空，不自动假定长度。
  配置保存到 `$XDG_CONFIG_HOME/teamagents/config.toml`（默认 `~/.config/teamagents/config.toml`），
  保留已有注释与设置，重启及新会话均可用。保存供应商后继续选择成员/模型才会切换；
  Codex 成员仅能使用支持 Responses 的自定义供应商。
  在线发现复用相应 profile 的地址、协议、认证和生成选项，仅覆盖模型 ID；需要供应商支持
  models 列表接口。选择器关闭后不保存在线目录，重新进入供应商会再次获取。
- **设置（`/settings`）**：居中浮层，只有一项可调——界面语言（语言行下方留一行空白，
  `Enter` 打开语言下拉、`Esc` 逐层关闭）；偏好保存在 `$XDG_STATE_HOME/teamagents/ui.json`。
  活动行 spinner 始终动画。
- **权限模式**：显示在页脚右下角——`Pre-authorized` 暗灰、`Full auto` 强调色粗体（均无底色）。
- **鼠标**：指针悬停在**页签或表格行**上时**只有该处**变为灰底白字（提示"可点"）；
  滚轮悬停在管理面板上等同 ↑/↓ 移动选中行，在聊天/日志上滚动内容；
  点击页签切换面板、点击表格行选中、点击聊天区回到输入框（浮层打开时忽略鼠标）。

| 操作 | 快捷键 |
|---|---|
| 发送 / 换行 | Enter / Shift+Enter 或 Ctrl+J |
| 调取输入历史 | 首行按 ↑，末行按 ↓；返回最新位置恢复草稿；历史跨会话与重启保留 |
| 行首 / 行尾 | Ctrl+A / Ctrl+E |
| 输入框 / 管理面板 | Tab 进面板（任意页签）；面板内 Esc 或 Tab 返回输入框；Ctrl+T 切页签、Ctrl+N 回输入框 |
| 打开设置 | 输入 `/settings` 回车；浮层内 ↑↓ 选择、Enter 切换、Esc 关闭 |
| 批准队列 | Ctrl+G |
| 请求停止 Leader | Esc；成员的其他工作继续 |
| 滚动对话 / 日志 | PgUp/PgDn 或 Ctrl+U/Ctrl+D（任何焦点下都滚动：团队等面板滚对话、日志面板滚日志），滚轮同样可用；Ctrl+Home/Ctrl+End 跳到最早/最新 |
| 日志成员筛选 | 日志面板聚焦时 ↑↓ 循环"全部 → 各成员 → 全部"，Enter 清除筛选；在团队面板高亮成员同样筛选日志 |
| 成员记录 | `/history` 浏览全部成员；团队/日志页签选中成员按 `h` 直接打开；记录页内 Enter 打开、`n/p` 分页、`r` 刷新、`i` 说明、Esc 返回 |
| 按词编辑 | Ctrl+W 或 Alt+Backspace 删词；Ctrl+←/Ctrl+→ 按词移动光标 |

原生成员在模型/工具边界响应停止；原生 Shell 还会轮询取消并终止隔离进程。正在阻塞的 HTTP/MCP
请求可能要等返回或超时；超过取消确认时限会显示结果不明，不承诺回滚文件或外部操作。
TUI 控制请求使用有界异步队列；后端卡住时请求会超时并显示错误，输入、导航和退出仍可用。

任务进入 BLOCKED 会唤醒等待者处理。Leader 可用 `cancel_task` 结清无需继续的任务，再按需求重新委派；用户也可在上区「任务」选中任务按 `c`。重新创建同名成员必须使用新的成员 ID，原身份不能复用。
