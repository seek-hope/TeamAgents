# 用户指南：配置、权限、恢复与故障处理

适用版本：当前 Rust 实现（2026-09-15 核对）。验收覆盖与尚未满足的方案要求见
[验收对照表](ACCEPTANCE.md)。

## 1. 配置

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
context_window = 128000   # 可选；填写后启用上下文自动压缩（见 §3.4），/status 也会显示占用比例
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

当前原生文件工具仍限制在成员工作目录和 `/artifacts/`，Shell 仍使用 bubblewrap，
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
- Shell 续用状态：同一个成员的 `cd` 与 `export` 会跨命令保留（像一个终端那样），状态存在
  `sessions/<id>/members/<成员>/shell/`，随会话保留、重开后仍生效；
  命令输出以 `[cwd: …]` 开头，模型据此知道下一条命令会从哪里开始。被中断/超时的命令不会更新
  状态，也不会留下半截文件（写入用临时文件 + rename）。
- 构建工具链只读镜像：`$HOME` 在沙箱里不可见，因此 sandbox 会把 `RUSTUP_HOME`（默认
  `~/.rustup`）与 `CARGO_HOME` 的 `bin`/`registry`/`git` 子目录只读挂到沙箱内
  `/tmp/.teamagents-toolchain/`，并设置对应环境变量。成员因此可以真的 `cargo build/test`
  （含离线 registry 缓存）；`credentials.toml`/`config.toml` 不挂载，所以注册表令牌不会
  进入沙箱。只有 Rust 已按此处理，其它语言的 HOME 级工具链（nvm/pyenv/…）仍不可见。
- 文件工具做符号链接与路径穿越防护，越界即拒绝。
- 写入用"进程内互斥 + 跨进程文件锁 + SHA-256 版本校验"：锁文件放在 `sessions/<id>/locks/`，
  另一个 teamagents 进程正在写同一路径时会等它写完；等待超过 10 秒则以
  `another teamagents process is writing this file` 拒绝。文件系统不支持建议锁时退化为原来的
  进程内互斥。
- 已绑定的 MCP 工具由 ChatRunner 直接调用，不再逐次批准。stdio 服务是白名单环境下启动的
  本机进程，当前没有 bubblewrap 的目录/网络隔离；其权限范围取决于该服务自身配置。
- “私有上下文隔离”是运行时投递与工具授权合约；本机 MCP 或 Codex 全自动执行可能按当前用户
  权限访问主机，不能承诺对恶意同用户进程的强保密隔离。

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
- **四类崩溃窗口**都有处理：提交后尚未启动、模型已完成但结果未归档、团队动作已提交但回执未落、
  外部工具已执行但结果未知。最后一种进入 `OUTCOME_UNKNOWN`，**不会**被当作成功或自动重试。
- 取消是“等待停止确认”，不是回滚：已产生的文件、请求与制品保留；等待中的批准失效后需重新批准。

### 3.1 会话记录在哪里、怎么管

```
$XDG_STATE_HOME/teamagents/sessions/<会话 id>/     # 默认 ~/.local/state/teamagents/sessions/
├── team.db            业务事实：动作回执、事件流、任务、回合、投递、批准、共享条目、拓扑补丁
├── artifacts/         长输出与制品：shell 输出超 200KB 落 exec-*.log（单个制品上限 64 MiB，超出的尾部丢弃并在输出里标注；目录总量超 512 MiB 时按时间删最旧的 exec-*.log，被删的旧引用再读会报文件不存在），工具结果用 /artifacts/xxx 引用
│                      （成员用 read_file/read_artifact/write_file 等按 /artifacts/ 前缀读写；ls/glob 与隔离 shell 看不到）
├── members/<成员>/work/  隔离/worktree 成员的专属工作目录（文件工具的执行根）
├── members/<成员>/chat_tree.json     对话树、当前 leaf、回退代次（含保留的旧分支）
├── members/<成员>/chat_history.json  当前线性历史快照及旧版迁移入口
├── members/<成员>/turns/<回合>.json  回合检查点（原工具调用 ID、执行进度、结果与已注入投递）
├── profiles.json      自动创建的会话级模型 profile
├── model_overrides.json  /model 设置的成员覆盖
└── session.lock       执行所有权（flock；同一会话同时只允许一个运行实例，kill -9 自动回收）
```

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
- **归档**：移动到 `sessions/archived/<id>/`，保留数据库原状态。
  TUI 的已归档行只展示，不支持切换、再次归档或删除，也没有取消归档命令。
- **删除**：使用会话面板 `d`，后端先检查运行锁及 worktree 的未提交/未合并成果，
  再清理目录（实现见 `engine/src/sessions.rs` 与 `engine/src/workspace.rs`）。
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

`/fork` 复制当前 TeamSpec 与 Leader 对话树到新会话，不复制团队事实、其他成员历史、
`profiles.json` 或 `/model` 覆盖；任意成员有 QUEUED/RUNNING 回合时拒绝分叉。
使用自动生成的会话 profile 的团队可能因此无法直接分叉，见验收表的已知差异。
`--plain` 使用 `rewind <节点 ID>`（无斜杠），不支持 fork；`model <成员>` 恢复默认模型。

### 3.3 磁盘占用与保留策略

- **制品**：Shell 长输出落 `artifacts/exec-*.log`，单个最多 64 MiB，整个目录超过 512 MiB 时
  新建制品会先按时间删最旧的 `exec-*.log`（自动，不需要配置）。
- **归档会话**：`teamagents sessions prune --days 30 [--dry-run]` 删除"最后更新超过 30 天"的
  归档会话；想让它自动发生就在用户配置里写 `[retention] archived_days = 30`（打开会话时清理，
  默认不写=不删）。两种方式都走同一套保护：运行中的会话、带未合并 worktree 成果的会话会跳过
  并报告原因。
- **会话数据库**：`teamagents sessions prune --history-days 30 [--dry-run]` 清理"已受理超过 30 天"的
  投递记录与对应事件（**未受理的投递和它需要的事件一定保留**，崩溃重放不受影响），清完自动 VACUUM；
  想自动发生就写 `[retention] history_days = 30`（打开会话时清理该会话，需要对上一次会话才有意义）。
  年龄按天算，默认不写=不清理——事件流也是 TUI 日志页签与审计的原料。
- **自动清理时保留**：回合检查点、对话树。它们分别是崩溃恢复、`/rewind` 与审计的
  依据；删掉旧检查点会让崩溃窗口内的"已完成回合"重新调用模型、可能重复投递，
  这个代价比省下的磁盘更贵。

### 3.4 上下文自动压缩（chat 运行时，D-28）

成员的模型上下文接近上限时自动压缩，无需任何命令；只对 chat 运行时生效
（codex 成员由 Codex 侧自行处理）。三层按成本递增：

1. **发送时限流**：工具结果字符串超过 50,000 字节时只截减**发给模型的那一份**（保留头尾各
   最多 25,000 字符、中间省略，并附 `read_history tool_call_id=…` 提示）。检查点与会话历史树
   保留完整原文，中间被省略的部分可用 `read_history` 按 tool_call_id 分页取回。
2. **视图遮蔽**：发给模型的消息里，较旧的工具输出替换为占位符（原文不动，
   仍留在会话历史树里）。
3. **阈值摘要**：`context_window` 已配置且最近一次 prompt 超过其 90% 时，额外调一次
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

每行内容：

| type | 字段 | 说明 |
|---|---|---|
| `session` | `session_id` | 会话已打开，后续行都属于它 |
| `tool` | `run_id`、`agent_id`、`tool`、`call_id`、`ok`、`error`、`arguments` | 每次工具调用的实时记录；`arguments` 是最长 500 字符的摘要（写大文件时会截断），据此可审计"改了哪个文件、跑了哪条命令" |
| `event` | `event`（核心事件：`run_started`、`goal_done`、`leader_reply`、`approval_requested` …） | 团队事务事件流水 |
| `result` | `status`、`exit_code`、`duration_ms`、`usage`、`verification` | 最后一行；`usage` 是各成员的真实 token 账本，`verification` 是 `--check` 命令的输出与退出码 |

| 现象 | 处理 |
|---|---|
| 界面提示“成员 leader 的模型 profile 'leader_main' 未配置” | 会话启动时即提示（TUI 会直接写出配置路径）。创建 `~/.config/teamagents/config.toml`（可复制 `examples/config.toml`），补 `[models.leader_main]` 后重开会话 |
| 输入后长时间没有回应 | 先看对话视图：回合失败会以 `✗ …回合失败：<原因>` 显示；若没有该行且状态栏“活动回合 ≥1”，说明模型正在生成（xhigh 思考可能较慢）。最常见原因是模型 profile 未配置或密钥环境变量缺失（`doctor` 可确认） |
| 报缺少某环境变量 | profile 的 `api_key_env` 指向的变量未导出；导出后重跑 |
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
- `teamagents validate` 离线检查 TeamSpec 结构、成员/通道/观察/共享引用及正数上限，
  只读取用户配置目录，不合并项目配置。任务依赖环、Codex 委派者限制在提交任务时检查；
  校验通过不代表模型端点或工具服务可用。
- 运行中 Leader 用 `apply_topology_patch` 的 `add_agent` 创建成员时，`model_profile` 可省略：
  系统会为该成员自动创建**同名会话级 profile**（复制 Leader 当前生效的模型配置，含 /model 覆盖后的
  模型与档位）；填一个未配置的名字则视为模型 ID（沿用 Leader 的连接）；填已有 profile 名则直接复用。
  自动创建的 profile 存 `sessions/<id>/profiles.json`，随会话持久、仅本会话可见，重开后仍生效；
  之后可用 `/model` 单独调整该成员（D-30）。
- 同一处 `add_agent` 还有两个默认行为（D-33）：省略 `tool_bindings` 时新成员**继承 Leader 的绑定**
  （显式写 `[]` 表示"只要团队工具"），并自动补上 `leader→成员` 与 `成员→leader` 两条 message 通道，
  所以 Leader 与成员一开始就能互相说话。成员之间不允许直接建通道：跨成员协作一律走共享空间，
  这样 Leader 与审计日志都能看到往来。
- 工作目录策略：`shared`（同一目录）、`isolated`（成员目录 + 明确输入/制品引用）、
  `git_worktree`（从明确提交建分支与 worktree；原目录脏时自动退回 shared 并说明原因；
  重开会话复用既有 worktree，未合并成果拒绝清理，删除会话同样受保护）。
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
- **改动审查（`v`）**：在团队页签（或日志页签）选中成员按 `v`，弹层显示该成员**最近一次编辑的
  diff**（`edit_file`/`edit_files`/`write_file` 的结果，绿色 `+`、红色 `-`）；`Esc` 关闭，
  `Ctrl+U/Ctrl+D` 或 `↑/↓` 滚动。还没有改动的成员会提示"还没有可审查的改动"。
- **日志页签**：除核心事件外，还实时显示每个成员实际调用的工具
  （`· edit_file alpha-fixer {"path":"alpha/alpha.py"}`，失败为 `✗`，参数截断到 120 字符）；
  按成员过滤时只显示该成员的工具行。日志在内存里只保留最新 2000 行（面板是环形缓冲）。
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
  `/status`（各成员 token 用量与上下文窗口占用）、`/model`（打开模型选择器，
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
  Chat 成员支持配置中的 OpenAI 兼容/DeepSeek/Anthropic 模型；Codex 成员只显示 OpenAI
  兼容配置，端点必须支持 Responses API。模型不支持所选档位时会显示服务端错误。
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
| 按词编辑 | Ctrl+W 或 Alt+Backspace 删词；Ctrl+←/Ctrl+→ 按词移动光标 |

原生成员在模型/工具边界响应停止；原生 Shell 还会轮询取消并终止隔离进程。正在阻塞的 HTTP/MCP
请求可能要等返回或超时；超过取消确认时限会显示结果不明，不承诺回滚文件或外部操作。
TUI 控制请求使用有界异步队列；后端卡住时请求会超时并显示错误，输入、导航和退出仍可用。

任务进入 BLOCKED 会唤醒等待者处理。Leader 可用 `cancel_task` 结清无需继续的任务，再按需求重新委派；用户也可在上区「任务」选中任务按 `c`。重新创建同名成员必须使用新的成员 ID，原身份不能复用。
