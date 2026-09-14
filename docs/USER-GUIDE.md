# 用户指南：配置、权限、恢复与故障处理

## 0. 版本适用性（先读）

仓库现在只有 Rust 实现（入口 `engine/target/debug/teamagents`）；Python 版已归档到 git 历史。
下表左列是**旧 Python 版**、右列是当前 Rust 实现；标 ⚠ 的是旧版专有、当前尚未移植的少数项
（其余能力均已实现）。

| 能力 | 旧 Python 版（已归档） | Rust（当前实现） |
|---|---|---|
| 用户配置 TOML | ✅ | ✅（`models`/`tools`/`skills_paths`/`instruction_files`；其余段落忽略） |
| 项目配置 `.teamagents/config.toml` | ✅ | ✅（同名用户定义优先；项目工具需 `[permissions] trust_project_tools = true`） |
| `[permissions]` 配置项 | ✅ | ✅（`mode` 与 `trust_project_tools`；`--full-auto` 仍可覆盖） |
| 模型接入 | langchain-* provider | OpenAI 兼容 HTTP + **Anthropic Messages API**（`protocol = "anthropic"`）；默认端点按 `provider`/`protocol` 解析（deepseek → api.deepseek.com/v1，anthropic → api.anthropic.com） |
| 内置工具 `files`/`shell` | ✅ | ✅ |
| `web_search`/`web_fetch` 绑定 | ✅ | ✅（`web_search` 目前只支持 `provider="anysearch"`） |
| MCP 工具服务 | ✅ | ✅ stdio + streamable HTTP 传输；工具名 `<service>_<tool>`，`tool_names` 过滤，绑定即授权 |
| Skills / AGENTS.md 注入 | ✅ | ✅（内容注入系统提示词，上限 8KB/文件、32KB/成员；Python 版是虚拟文件系统） |
| `workspace_policy` | shared / isolated / git_worktree | ✅ 三者齐全（worktree 复用、脏仓库回退 shared 并说明、未合并成果拒绝清理；删除会话同样受保护） |
| 会话锁 | flock | flock（`File::try_lock`；kill -9 自动回收，文件里的 pid 仅作诊断） |
| TeamSpec 导入 | JSON / YAML | JSON / YAML（`--team` 与 `validate` 均可） |
| TUI | Textual | ratatui（Rust 原生设计：固定上下分区、六页签、`/settings` 浮层；键位见文末） |
| deepagents 子代理 / 图框架 | ✅ | ⚠ 未移植（`ChatRunner` 工具循环取代；`general-purpose` 子代理没有等价物） |

## 1. 配置

### 1.1 位置

| 内容 | 位置 |
|---|---|
| 用户配置（模型 profile、工具绑定、Skills 目录、指令文件） | `$XDG_CONFIG_HOME/teamagents/config.toml`（默认 `~/.config/teamagents/config.toml`） |
| 项目配置（两版；同名条目用户定义优先，不能开启全自动或扩大预授权） | `<项目>/.teamagents/config.toml` |
| 会话状态（业务库、成员私有检查点、制品、成员工作目录） | `$XDG_STATE_HOME/teamagents/sessions/<session_id>/` |
| TUI 语言偏好 | `$XDG_STATE_HOME/teamagents/ui.json`（默认 `~/.local/state/teamagents/ui.json`） |
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
context_window = 128000   # 可选；填写后启用上下文自动压缩（见 §3.3），/status 也会显示占用比例
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

旧式 `mcp_transport = "sse"` 已从 MCP 规范移除，绑定会直接报错并提示改用 `"http"`。

内置能力名 `files` / `shell` / `web` 不需要配置条目：成员在 TeamSpec 里引用即可。

### 1.4 Skills 与指令文件（两版；Rust 版为提示词注入）

```toml
skills_paths = ["~/.agents/skills", "~/.config/teamagents/skills"]
instruction_files = ["~/.config/teamagents/AGENTS.md"]
```

项目根的 `AGENTS.md` 会自动作为指令文件加载。Skills 不授予任何新权限。

Skills 的加载与分发（Rust 版，对应方案 §12.1 的“发现 + 按需读取”）：

- **发现/按需读取**：成员在 `tool_bindings` 里绑定 `skills` 即获得 `skill` 工具
  （`action="search"` 按关键词检索名称+简介，`action="read"` 按名取全文）。注册根就是
  `skills_paths`，只读；项目 `.teamagents/skills` 在工作区内，用 files 工具即可读。
- **分发**：TeamSpec 或 topology patch（`add_agent`/`update_agent`）里的成员 `skills: [名称]`
  会把对应 SKILL.md 内容注入该成员系统提示词（8KB/文件、32KB/成员上限；同名按
  用户级 → 项目级 → 成员级覆盖）。Leader 据此把泛用/专精技能分给特定成员；
  未点名的技能不再注入（技能库大时全量注入必然超上限）。

## 2. 权限

### 2.1 两种模式

- **approved_scope（默认）**：预授权范围内自动执行；范围外生成批准请求。
  初始预授权 = 当前工作目录读写、被绑定的工具（文件/搜索/MCP）、**无网络**的隔离 Shell。
- **full_auto（仅用户可开启）**：跳过逐次批准，仍保留团队通信 ACL、动作校验、记录与执行上限；
  不绕过操作系统与外部服务的限制。TUI 状态栏始终显示当前模式。

切换：TUI `Ctrl+F`，或 `teamagents --full-auto`，或用户配置
`[permissions] mode = "full_auto"`（两版均支持；非法取值会直接报错，doctor 可见）。项目配置**不能**开启全自动。
Rust 版以会话行为准：审批门每次调用前读取会话的权限模式，所以切换立即生效、无需重开。

### 2.2 批准语义

- 批准绑定**具体操作与参数**；参数变化需要重新批准。
- 提供三种决定：本次批准（once）、会话内批准（session）、拒绝。
- **once 批准在执行后即消费**（置 EXPIRED），同一操作再次执行需重新批准；session 批准（以及尚未
  消费的 once 批准）只在策略修订（revision）不变时放行；EXPIRED 的记录按“需重新请求”处理，
  而不是放行。
- 等待批准只暂停相关操作，其他成员继续；`WAITING_APPROVAL` 不算回合结束。
- 恢复时重新核对参数、配置版本与权限，历史批准不会沿用失效范围。
- Rust 版 Codex 成员的批准等待有 600s 上限，超时把该批准置 EXPIRED（需重新批准）；其他成员不受影响。

### 2.3 隔离边界（诚实说明）

- Shell 走 bubblewrap：只挂载系统只读目录 + 授权工作目录，隔离 PID/网络/临时目录；
  网络默认关闭，需要联网的操作要批准。Rust 版**要求** bwrap：缺失时命令直接失败
  （`IsolationUnavailable`），不会退化成不隔离执行；命令环境是白名单（不含模型密钥）。
- 文件工具做符号链接与路径穿越防护，越界即拒绝。
- “私有上下文隔离”是运行时投递与工具授权合约；full_auto 允许程序按当前用户权限访问主机，
  不能同时承诺对恶意同用户进程的强保密隔离。

## 3. 恢复

- 正常退出默认保存并暂停；异常退出后下次启动自动恢复：团队版本、待办任务、消息位置、
  成员私有线程与批准队列都会重新装载（Rust 版 ChatRunner 的成员对话历史落盘在
  `members/<成员>/chat_history.json`，重启后装载）。
- 执行意图（`QUEUED` 回合）先持久化再执行，恢复后继续；ChatRunner 在模型响应、工具调用与
  结果边界保存回合检查点，恢复时沿用原工具调用 ID，通过核心回执去重。Codex 回合先核对外部历史。
- 旧版遗留的执行中 Chat 回合若缺少有效检查点，或外部工具已开始但没有落盘结果，进入
  `OUTCOME_UNKNOWN`，需核对已产生的结果再决定后续任务；已完成的旧版对话历史继续兼容。
- **四类崩溃窗口**都有处理：提交后尚未启动、模型已完成但结果未归档、团队动作已提交但回执未落、
  外部工具已执行但结果未知。最后一种进入 `OUTCOME_UNKNOWN`，**不会**被当作成功或自动重试。
- 取消是“等待停止确认”，不是回滚：已产生的文件、请求与制品保留；等待中的批准失效后需重新批准。

### 3.1 会话记录在哪里、怎么管

```
$XDG_STATE_HOME/teamagents/sessions/<会话 id>/     # 默认 ~/.local/state/teamagents/sessions/
├── team.db            业务事实：动作回执、事件流、任务、回合、投递、批准、共享条目、拓扑补丁
├── checkpoints.sqlite ⚠ Python 版：成员私有线程（LangGraph 检查点）
├── artifacts/         长输出与制品：shell 输出超 200KB 落 exec-*.log，工具结果用 /artifacts/xxx 引用
│                      （成员用 read_file/read_artifact/write_file 等按 /artifacts/ 前缀读写；ls/glob 与隔离 shell 看不到）
├── members/<成员>/work/  隔离/worktree 成员的专属工作目录（两版同路径，文件工具的执行根）
├── members/<成员>/chat_history.json  Rust 版：成员对话历史（重启后装载）
├── members/<成员>/turns/<回合>.json  Rust 版：回合检查点（原工具调用 ID、执行进度、结果与已注入投递）
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

### 3.3 上下文自动压缩（chat 运行时，D-28）

成员的模型上下文接近上限时自动压缩，无需任何命令；只对 chat 运行时生效
（codex 成员由 Codex 侧自行处理）。三层按成本递增：

1. **写入时限流**：单个工具输出超过 50K 字符时保留头尾、中间省略。
2. **视图遮蔽**：发给模型的消息里，较旧的工具输出替换为占位符（原文不动，
   仍留在会话历史树里）。
3. **阈值摘要**：`context_window` 已配置且最近一次 prompt 超过其 90% 时，额外调一次
   模型把对话压成结构化摘要（目标/进展/文件/错误/任务/下一步）。**被压缩的原文不丢失**：
   仍在历史树中，`/rewind` 回退到压缩前节点即可再见全文；成员也可用 `read_history`
   工具按 tool_call_id 取回某次工具调用的原始输出。压缩连续失败 3 次后本会话自动停试
   （不影响回合本身，超限的 API 报错会原样暴露）。

## 4. 故障处理

| 现象 | 处理 |
|---|---|
| 界面提示“成员 leader 的模型 profile 'leader_main' 未配置” | 会话启动时即提示（TUI 会直接写出配置路径）。创建 `~/.config/teamagents/config.toml`（可复制 `examples/config.toml`），补 `[models.leader_main]` 后重开会话 |
| 输入后长时间没有回应 | 先看对话视图：回合失败会以 `✗ …回合失败：<原因>` 显示；若没有该行且状态栏“活动回合 ≥1”，说明模型正在生成（xhigh 思考可能较慢）。最常见原因是模型 profile 未配置或密钥环境变量缺失（`doctor` 可确认） |
| 报缺少某环境变量 | profile 的 `api_key_env` 指向的变量未导出；导出后重跑 |
| 命令因“refusing to run without isolation”失败 | 安装 bubblewrap；不要以降低隔离来绕过 |
| Codex 成员卡在“Reconnecting” | codex 的 provider 凭据不可达：检查 `~/.codex/config.toml` 的默认 provider 与密钥，或给成员配置走环境变量密钥的 profile |
| 回合因 `LIMIT_REACHED` 停止 | 达到目标回合数或模型请求步数上限（`limits.max_model_steps_per_turn` 真实约束模型请求数）；该回合记为 FAILED 并发 `limit_reached` 事件，调整 `limits` 后可继续 |
| 成员回合活动超时 | 超过 `limits.turn_active_timeout_s`（默认 1200s）会中断成员回合（不再继续执行）；回合记为 FAILED，按需重派任务 |
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
  `git_worktree`（从明确提交建分支与 worktree；原目录脏时自动退回 shared 并说明原因；
  重开会话复用既有 worktree，未合并成果拒绝清理，删除会话同样受保护）。
  三者在两版均已实现（Rust 见 `docs/DECISIONS.md` D-19）。

## TUI（Rust 版布局与交互）

> Python（main）版是 Textual 界面；Rust 版按 D-20 重新设计（固定分区、滚动、胶囊状态、
> `/settings` 浮层），以终端习惯为准。

- **固定上下分区**：第 1 行是状态行（`TeamAgents「应用名」+ 会话 id + ⚠ 待批准 + ▸ 未完成`），
  上方是管理面板框（占约 2/5 高度、整宽），下方是聊天区（活动行、对话、流式预览、输入框），
  最后一行是页脚键位。分区与宽度无关，按键位置恒定。
- **管理面板**：六个页签（团队/任务/共享空间/批准/会话/日志），待办类页签带计数
  （如 `Tasks 2`、`Approvals 1`），选中页签用 `▍` 标出；面板底部一行是该面板的按键说明（英文）。
  表格随选中行滚动，列在窄终端下按优先级收起；空列表显示"— No tasks yet —"之类的提示。
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
  覆盖只在当前会话生效，不改 TeamSpec）、`/rewind`（列出可回退点，`/rewind <序号>` 回退到该条之前，
  `/rewind 0` 清空对话；被放弃的分支仍保留在树里，可再次回退；Leader 回合进行中不可回退）、
  `/fork`（从当前 Leader 对话分叉为新会话，团队任务/事实不复制；回合进行中不可分叉）。
- **模型切换**：选择配置会一起切换 API 地址、认证环境变量、协议和上下文窗口，从下一回合生效，
  当前回合继续运行并可正常取消。配置只在当前打开的会话内覆盖，重开恢复默认。
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

原生成员在模型/工具边界响应停止。已经执行中的工具要先返回，界面显示停止请求；超过确认时限会显示结果不明，不承诺回滚文件或外部操作。

任务进入 BLOCKED 会唤醒等待者处理。Leader 可用 `cancel_task` 结清无需继续的任务，再按需求重新委派；用户也可在上区「任务」选中任务按 `c`。重新创建同名成员必须使用新的成员 ID，原身份不能复用。
