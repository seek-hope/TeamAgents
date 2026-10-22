# TeamAgents

**你只和 Leader 说话，Leader 现场组队。** 运行在 Linux 终端上的团队式 Agent 产品：
你提出目标，Leader 按需招募工作实例、派发任务、协调协作、汇总结果；你负责看进度、在越权时点批准。

- **唯一权威状态**：每会话单 SQLite（WAL + `synchronous=FULL`），实例/任务/授权/预算/批准/回执与事件
  同事务提交；崩溃恢复按持久化位置分类，不猜测重放。
- **一个 daemon 拥有会话**：TUI 与 `exec` 都是它的薄客户端（Unix socket JSON 协议，断线按事件水位续读）。
- **受控协作**：`spawn`/`delegate`/`send`/`wait` 全部经控制平面授权与派发线性化点重查。
- **多供应商**：同一会话内可混用 DeepSeek（chat-completions）、Responses、Anthropic 三类协议的真实实例。

## 文档

| 文档 | 内容 |
|---|---|
| [使用指南](docs/USER-GUIDE.md) | 上手、配置、权限、Skills/MCP、恢复与清理 |
| [日常编码成熟度与产品对比](docs/PRODUCT-COMPARISON.md) | 当前 Pi 对比、证据边界、按优先级排列的候选改进，以及历史 Codex 对比 |
| [验收对照表](docs/ACCEPTANCE.md) | A01–A36 逐项证据与已知缺口 |
| [能力与证据](docs/CAPABILITIES.md) | 每项能力一行：背后的检查、该检查量出的边界、以及仍未决的部分 |
| [开发与维护](docs/DEVELOPMENT.md) | 工具链、门禁、目录约定、复跑命令 |
| [设计与验收基线](docs/DESIGN.md) | 已确认需求、架构与协议约束、验收矩阵 A01–A36、完成定义 |
| [决策记录](docs/DECISIONS.md) | 用户确认的方向、边界与偏离记录 |
| [安装指南](docs/INSTALL.md) | 下载安装与升级 |
| [形式化验证](verification/README.md) | TLA+ 规格与 Kani 证明：性质 ↔ 代码 ↔ 验收编号、未证明清单 |

> **首次使用：** [安装指南](docs/INSTALL.md)。首个公开版本为 **v0.2.0**（Linux x86_64）。

## 怎么工作

```
你 ──目标（自然语言）──▶ Leader ──spawn / delegate──▶ 工作实例 A / B / C（可并行）
                          │  ▲                            │
                          │  └────────── send ────────────┘  实例之间不私聊，
                          │                    协作只走控制平面授权的消息与共享空间
                          ├── 权限 / 批准请求 ──▶ 你（只在越权或外网访问时打扰你）
                          └── finish：Leader 声明目标达成并通过完成检查，才算完成
```

- TUI 固定与 Leader 对话；组不组队、组几个人由 Leader 决定（一人成队合法）。开发者 CLI 也支持直接向成员发送指令和干预。
- 每个实例有自己的模型、工具绑定、工作区策略与预算；越权操作变成**批准请求**，其他实例继续干活。
- 运行时事实（谁在做什么、任务卡在哪、回合为什么结束）都落在会话库里：可查、可恢复、可审计。

## 主要特性

- **受控协作**：`spawn` / `delegate` / `send` / `wait` 都经控制平面授权，并在派发时重查权限版本；会话启动时默认把团队权限（会话范围的 `manage` / `delegate` / `message`）授给 Leader，授权可随时撤销；工具面跟着授权走：实例只会看到自己真正能派发的工具（Leader spawn 的 worker 在拿到 `shell@workspace` 之前没有 shell）。
  D-455 支持从用户显式授予的可下授权限派生任务级子授权；缺少权限时形成待批准请求。父级撤销级联、超时与未知结果都有恢复分类。
- **工作区策略**：`spawn` 可按需给实例共享目录、私有隔离目录或独立 Git worktree（各自分支）；
  终止实例时按记录回收，有未提交/未合并成果的目录永不自动删除，只报告原因。
- **多模型混合**：同一会话可混用三种线上协议（responses / anthropic / chat-completions）的真实实例，
  各自的模型、档位与原生上下文窗口独立配置；`spawn` 可指定 worker 用哪个 catalog 条目（键或模型名），
  一个团队可以混用条目。条目不认识时该工具调用直接报错；永远起不来的实例会被 park 并给出原因，不会拖住整个会话。
- **唯一权威状态**：每会话单 SQLite（WAL + `synchronous=FULL`）。崩溃恢复按持久化位置分类——
  已知结果复用、在途丢失诚实记账（`OUTCOME_UNKNOWN`）、**不猜测重放**。
- **权限门**：`[permissions] sandbox` 选两种模式：`none`（默认：主机 Shell，无 shell 审批，D-466）或
  后端（`bubblewrap`/`docker`/`ssh`，越权需批准）；一条批准绑定一个具体操作、其参数散列与权限
  revision，只对那一次派发生效。
- **完成检查闸门**：`finish` 只接受诚实结论；目标成功结算前必须真实通过的是**你自己**配置里的 `[[checks]]`。
  可信项目配置已经会被加载，但不能提供验收检查；验收策略始终由用户掌握（见下文「配置与团队」）。
- **长上下文压缩**：按实际窗口占用触发，摘要保留原始要求、用户修订、验收与未决问题；
  原文经 `read_history` 仍可检索，压缩调用计入目标预算。
- **工具面**：文件读写/搜索（精确匹配编辑、SHA-256 版本校验、原子写入，进程内写锁串行化）、
  会话内持久 shell（`cd`/`export` 跨命令保留）、
  网页搜索与抓取（DeepSeek Search/Fetch）、MCP（stdio 与 streamable HTTP）、Skills（`~/.agents/skills`）。
- **用户钩子（`[hooks]`）**：`notify` 把事件（`tool_call`/`team_action`/`run_*`）通知给你自己的程序；
  `pre_tool` 能在任何原生工具执行前拦截（exit 2 拒绝，stderr 作原因），坏钩子只记日志不卡团队。
- **隔离**：`sandbox = "none"`（默认）明确在主机执行；配置了后端时实例 shell 走该后端，后端不可用会
  明确报错，不会静默退化成主机执行。

## 安装

要求：Linux（x86_64）；沙箱后端可选（默认 `none`）；模型密钥从环境变量读，不写进配置。

### 安装已发布版本

首个公开版本为 **v0.2.0**（Linux x86_64，musl 静态包，无需 Rust 工具链）：

```bash
(
  set -eu
  installer="$(mktemp)"
  trap 'rm -f "$installer"' EXIT
  curl -fsSL https://raw.githubusercontent.com/seek-hope/TeamAgents/main/install.sh -o "$installer"
  sh "$installer"
)
export PATH="$HOME/.local/bin:$PATH"
teamagents init
```

安装器会解析最新 release、校验发布页的 `SHA256SUMS`，并一起安装 `teamagents` 与 `teamagents-tui`；
再次运行会升级并保留现有配置和会话。`--version`、`--bin-dir` 与本地归档用法见[安装指南](docs/INSTALL.md)。

D-451 源码新增显式文本/图片附件（`/attach PATH` 或 `exec --attach PATH`）、工作区路径补全、
待消费输入的修订和取消（`/queue` 或 `inputs`）、会话浏览器搜索以及完整的本地 Markdown/JSON 导出。
展示解析在有资源和超时限制的 Rust 辅助进程中运行，失败时保留源码。使用方式见
[用户指南](docs/USER-GUIDE.md)，确定性本地验证和剩余缺口见
[成熟度报告](review/maturity-2026-10-11/REPORT.md)，其中证据只对应其固定源码。当前源码还包含外部编辑器、
任务级能力下授、可选的操作自动审查与供应商缓存统计。当前首个公开发行版为 v0.2.0。

### 从源码构建

```bash
cargo build --locked --release --manifest-path engine/Cargo.toml --bin teamagents
cargo build --locked --release --manifest-path tui/Cargo.toml --bin teamagents-tui
mkdir -p ~/.local/bin
install -m755 engine/target/release/teamagents tui/target/release/teamagents-tui ~/.local/bin/
export PATH="$HOME/.local/bin:$PATH"
```

需要 Rust 工具链，版本由 `rust-toolchain.toml` 固定；依赖已缓存时可加 `--offline`。
构建完成后使用下面的 `teamagents init` 初始化配置。

## 快速开始

默认 `sandbox = none` 不需要沙箱。若要用 bubblewrap 后端，先安装它（Debian/Ubuntu：
`sudo apt install bubblewrap`；其他发行版见安装指南），然后在同一个终端中执行：

```bash
teamagents init                       # 创建内置最小配置；已有配置不会覆盖
export DEEPSEEK_API_KEY='你的模型密钥'  # 默认配置使用 DeepSeek；其他服务按配置中的 api_key_env 设置
teamagents doctor                    # 自检：配置 / 密钥 / 状态根 / 技能路径 / 隔离探针
teamagents --cwd /path/to/project     # 换成实际项目目录；省略 --cwd 则使用当前目录
```

`init` 遵循 XDG 配置目录；默认使用 DeepSeek Flash（1M 上下文），其他服务可编辑生成的 TOML。
已有配置会被保留；请用 `doctor` 检查，不要假定 `init` 会合并新增配置字段。

进去以后直接说目标，例如：

```
把 /tmp/proj 里的测试修到全绿，改完说明每个文件为什么这么改。
```

Leader 会自己决定要不要组队、组几个人、谁干什么；需要你拍板时（越权限的命令、外网访问）
才会弹出批准请求。TUI 固定与 Leader 对话，`Ctrl+A` 处理待批准请求；开发管理和诊断入口见 `teamagents --help`（D-445）。

脚本 / CI 用无头入口（`exec` 与 TUI 共用同一个 daemon；提示词写 `-` 就从 stdin 读，长指令可直接管道进来）：

```bash
teamagents exec "1+1 等于几？直接回答"                           # 提交一次输入，打印回复
teamagents exec --json --timeout 180 "把 /tmp/proj 的测试修绿"    # 机器可读摘要
teamagents exec --stream-json "改到测试全绿" | jq -c '.event.kind' # 运行期间实时看事件
teamagents exec --check "cargo test --offline" "改到测试全绿"     # 追加你自己的验收命令
teamagents exec --accept tests="pytest -q" "修好失败的测试"    # 该检查通过前 goal 不能结算
git diff | teamagents exec -                                    # 提示词从 stdin 读
```

`--stream-json` 每行向 stdout 写一个 JSON 对象：运行期间观察到的每个会话事件写一行
`{"type":"event","event":{…}}`（按日志顺序、每个只出现一次、写出即 flush），最后一行是报告
`{"type":"report","report":{…}}`，与 `--json` 打印的是同一个对象。退出码不变；消费方提前关闭管道（如
`| head`）只会结束事件流，不会影响这次运行。

退出码：`0` 已结算，`1` 失败或未完成，`3` 有操作在等批准（无头运行没人能批，因此立刻返回而不是干等），
`124` 超过 `--timeout`，`2` 用法或环境错误（没有 daemon、没有模型 profile）。每个 `--check COMMAND`
在回合结束后按顺序在隔离 shell 里、在你的工作目录（`--cwd` 或当前目录）执行；第一条失败即停止，整个运行
判为失败。判定结果会打印出来、写入 `<state root>/verification.json`，并作为 `verification` 出现在
`--json` 报告里。而 `--accept ID=COMMAND` 则在输入落地之**前**挂到 *goal* 上（`teamagents goals open --check ID=COMMAND` 对你自己打开的 goal 做同一件事），因此运行时会在完成边界执行它，并在 goal 结算前把失败的回合送回去继续工作——这正是 `[[checks]]` 所喂的同一套机制（D-385）。

**会话的上限由你定。** 配置里的 `[limits]` 给每个 goal 设上限：`max_total_tokens` 会在用量到达上限前拒绝
新请求，`deadline_minutes` 会在超过截止时间后拒绝新请求；两者都不配置时，会话会一直跑到你手动停止为止（`doctor`
会把这件事直接说出来）。见 [docs/USER-GUIDE.md](docs/USER-GUIDE.md) §2.2。

**能力由你来发。** worker 不会隐式继承权限。你可以单独授予它共享工作区 shell，
也可以显式允许 Leader 为声明需要该能力的任务下授权限：

```bash
teamagents authority                                    # 授权与待批准能力请求
teamagents authority grant --subject i-worker-1 --action shell --scope workspace
teamagents authority grant --subject i-leader --action shell --scope workspace --delegable
teamagents authority revoke --grant g-1a2b3c4d          # 撤销是终局的，派生授权一并撤销
```

可下授权限是用户的选择，不是默认值。子授权不能扩大父授权范围或再次下授，随任务结束或实例终止失效。
没有覆盖范围的父授权时，已声明的能力需求会等待用户决定，而不是先启动一个无法执行任务的 worker。

每次派发都会重新校验授权，所以撤销也能拦住已经排队的工作，工具会在该实例下一次请求时从模型可见的工具面
消失。表面会拒绝那些「永远做不了任何事」的授权（动作不在词表内，或像 `shell@instance:i-worker-1` 这样没有
任何检查会问到的组合），并告诉你该用哪个资源范围。完整动作词表、退出码见
[docs/USER-GUIDE.md](docs/USER-GUIDE.md) §3.1。

## 常用参数与键位

| 用法 | 含义 |
|---|---|
| `--cwd DIR` | 以 DIR 为工作目录（默认当前目录）；只对这条命令启动的会话生效——已在运行的会话保留自己的工作目录，客户端会把它打印出来 |
| `--state-root PATH` | 指定状态根/基目录（默认 `$XDG_STATE_HOME/teamagents/v2`）；默认会话直接用它，命名会话放在其 `sessions/<id>/` 下 |
| `--session ID` | 附着到该基目录下登记的命名会话（D-364）；`teamagents sessions` 列出 id。它解析为该会话自己的状态根，TUI、`exec`、`daemon`、`init`、`doctor` 都可用 |
| `--model KEY` | 选择模型目录键（默认 `leader_main`） |
| `--every MINUTES` / `--prompt TEXT` | `automations add`：周期与每次运行提交的提示词（D-367） |
| `--query TEXT` / `--limit N` | `sessions search`：要查找的子串与最多返回的命中数（默认 20，D-368） |
| `--full-auto` | 用户显式开启全自动（仅用户可开；默认越权时请求批准）；只对这条命令启动的会话生效——已在运行的会话保留它启动时的模式，客户端会把这个模式打印出来而不是假装生效 |
| `init` / `doctor` / `daemon` / `daemon --stop` / `exec` / `authority` / `approvals` / `instances` / `tasks` / `runners` / `artifacts` / `sessions` / `automations` / `version` / `--help` | 准备配置与状态根 / 自检 / 单独运行 daemon（输出写入 `<state root>/daemon.log`）/ 停止拥有该状态根的会话，以套接字为地址（不依赖 pid；`--cwd`/`--model`/`--full-auto` 会被拒绝）/ 无头输入 / 列出、发放与撤销能力 / 列出并决定待批准操作 / 暂停、恢复、打断当前回合、终止、合并 worktree 成员的分支并列出实例 / 列出并取消任务 / 列出该状态根仍持有的作业 runner 并让已结束的那些退休 / 列出该状态根的产物（字节数、归属、是否落盘）并回收无人引用的那些（`runners stop --service --yes` 则改为对已结算命令遗留的进程组发信号）/ 列出、创建、分叉、重命名、恢复、搜索、归档与删除基目录下的命名会话（D-364/D-365/D-368） / 列出、创建、暂停、恢复与删除“自动开始目标”的定时任务（D-367） / 版本 / 用法 |

对话支持 Markdown、代码块、列表和表格，用户消息与工具结果分层显示。`Ctrl+O` 展开或折叠工具预览，完整记录仍可在会话页查看（D-447）。
Mermaid、SVG、DOT 图表和本地 PNG/JPEG/GIF/WebP/BMP/TIFF/ICO 图片也可在对话与会话历史中显示。
`Ctrl+G` 切换预览与源码。Kitty 显示图片，其他终端使用字符图或彩色块预览；
`TEAMAGENTS_GRAPHICS=auto|kitty|blocks|off` 可选择显示模式（D-448）。
D-450 增加语言语法高亮，并仅解码可见媒体，使长历史中后面的图片也能加载。
输入斜杠前缀显示本地命令候选：上下键选择，`Tab` 或 Enter 填入，`Esc` 保留草稿并关闭候选。
填入后按 Enter 才执行命令。

TUI 键位：`Enter` 发送给 Leader，`Ctrl+J` 或 `Shift+Enter` 换行，`↑`/`↓` 回顾输入，
`PageUp`/`PageDown` 和滚轮浏览对话。`Ctrl+A` 聚焦待批准请求，`a` 批准本次，`d` 拒绝，
`Esc` 返回草稿。`Ctrl+C` 中断 Leader 正在等待的模型请求（已运行的工具和委派工作继续），
`Ctrl+D` 退出界面，会话继续运行。`Ctrl+S` 或 `/sessions` 打开会话列表和历史：方向键选择，
Enter 切换，方括号翻页，`d` 后按 `y` 确认删除，Esc 取消或返回。删除当前会话成功后退出 TUI。
界面命令包括 `/help`、`/sessions`、`/interrupt`、`/quit`、`/attach`、`/detach`、`/queue` 和 `/edit`。
`Ctrl+E` 用 `VISUAL` 或 `EDITOR` 打开草稿；成功后回到输入框，不会自动发送。
在草稿末尾加 `/edit` 也会打开编辑器；单独输入 `/edit` 显示说明，它尚未列入补全菜单。
各会话草稿分别保留，批准请求不会抢走输入焦点。

开发者可用 `teamagents status --json` 检查会话，
`teamagents history --id INSTANCE --limit 400 --json` 读取成员最近的已存储消息和工具记录。
成员、任务、目标、权限、会话和 MCP 管理均由 CLI 提供，见[命令地图](docs/USER-GUIDE.md#1-quick-start)。

## 配置与团队

- 会话读取你的 `$XDG_CONFIG_HOME/teamagents/config.toml`，并向上查找最近的 `.teamagents/config.toml`。
  项目设置**默认不起作用**；你可在 `[permissions] trusted_projects` 中显式信任规范化后的项目根目录，
  或用 `trust_project = true` 信任所有项目。随后 models、tools、skills 路径与 instruction 文件才会合并
  （同名时用户定义优先）；权限、hooks、checks、retention、limits 与更新策略始终属于用户。
  `doctor` 报告信任和合并结果。指令文件仍须显式配置；发现项目配置不等于自动加载 AGENTS.md。
  shell 后端可选 bubblewrap、Docker 或 SSH，边界见使用指南。
- 模型 profile 用 `protocol = "responses" | "anthropic" | "openai" | "deepseek"` 选线上格式，
  `base_url` + `model` 决定实际接哪家；密钥只写环境变量名。
- 用户配置里的 `[[checks]]` 是"完成验收"的机器契约（只允许写在用户配置，项目文件不能定义）：
  目标声称完成时运行时会在会话的 shell 模式里执行这些命令（`sandbox = none` 即主机），检查不过就打回
  修复，而不是让总结直接算成功。
- 组队由 Leader 在运行时按目标决定（`spawn`/`delegate`/`send`/`wait`），没有静态团队定义文件。
- 工具绑定即授权、批准语义、Skills/MCP、会话保留策略、故障处理：见
  [用户指南](docs/USER-GUIDE.md)。

## 现状与限制

- 支持的分发目标是 Linux x86_64；首个公开发行版为 v0.2.0。没有已支持的 Windows/macOS 构建、分布式成员协议或内置浏览器自动化。
- 选定的 bubblewrap、Docker 或 SSH 后端不可用时会拒绝执行，不会静默退回本地主机。
  SSH 是远程 shell，不是分布式 agent；它不保证远端进程取消，也不能强制远端网络隔离。
- 可以开展受控的日常编码试用，但尚未证明能够作为唯一通用主力平替 Pi。
  CLI/TUI 已提供只读工作诊断与显式恢复指引（D-461），不会自动催促模型。
  后续优先完善开发环境与完整 Skills 工作流、配对编码评估；随后是指令发现、工具组合、会话操作和完整计费。
- DESIGN §7 的四类协议族（chat-completions——`openai` 与 `chat/completions` 两个名字、DeepSeek 扩展、
  Anthropic、responses）都有本地假服务回归测试（`engine/tests/providers_fake.rs`），并由
  `python3 review/dogfood/protocols.py` 逐个对真实服务验收。环境中没有对应凭据的协议族会报告为**跳过**
  （`--strict` 会把跳过变成失败），因此实际能验收几类取决于运行机器的凭据（D-151）。
- 验收边界与证据见 [验收对照表](docs/ACCEPTANCE.md)，未做项与已知天花板在那里逐条列明。

英文版见 [README.md](README.md)；本文件是仓库里唯一允许出现中文的文件。

## 开发

开发先运行 `make check`；完整流程见 [开发与维护](docs/DEVELOPMENT.md)，仓库约定见 [AGENTS.md](AGENTS.md)。
缺陷、审查与评测证据记录在 `review/`，形式化验证证据在 `verification/`。
打 `v*` tag 会构建发行包；v0.2.0 已发布。后续发布仍由仓库变量 `TEAMAGENTS_RELEASE_APPROVED` 控制。
