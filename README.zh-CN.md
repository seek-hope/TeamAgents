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
| [与 Codex CLI、Pi、Hermes 的对比](docs/PRODUCT-COMPARISON.md) | 带日期与出处的对比快照，以及它引出的决策 |
| [验收对照表](docs/ACCEPTANCE.md) | A01–A36 逐项证据与已知缺口 |
| [开发与维护](docs/DEVELOPMENT.md) | 工具链、门禁、目录约定、复跑命令 |
| [设计与验收基线](docs/DESIGN.md) | 已确认需求、架构与协议约束、验收矩阵 A01–A36、完成定义 |
| [决策记录](docs/DECISIONS.md) | 用户确认的方向、边界与偏离记录 |
| [安装指南](docs/INSTALL.md) | 下载安装与升级 |
| [形式化验证](verification/README.md) | TLA+ 规格与 Kani 证明：性质 ↔ 代码 ↔ 验收编号、未证明清单 |

> **首次使用：** [下载安装与升级](docs/INSTALL.md) · [最新发行版](https://github.com/seek-hope/TeamAgents/releases/latest)

## 怎么工作

```
你 ──目标（自然语言）──▶ Leader ──spawn / delegate──▶ 工作实例 A / B / C（可并行）
                          │  ▲                            │
                          │  └────────── send ────────────┘  实例之间不私聊，
                          │                    协作只走控制平面授权的消息与共享空间
                          ├── 权限 / 批准请求 ──▶ 你（只在越权或外网访问时打扰你）
                          └── finish：Leader 声明目标达成并通过完成检查，才算完成
```

- 你**总是**和 Leader 对话，不直接指挥成员；组不组队、组几个人由 Leader 按需求决定（一人成队合法）。
- 每个实例有自己的模型、工具绑定、工作区策略与预算；越权操作变成**批准请求**，其他实例继续干活。
- 运行时事实（谁在做什么、任务卡在哪、回合为什么结束）都落在会话库里：可查、可恢复、可审计。

## 主要特性

- **受控协作**：`spawn` / `delegate` / `send` / `wait` 都经控制平面授权，并在派发时重查权限版本；会话启动时默认把团队权限（会话范围的 `manage` / `delegate` / `message`）授给 Leader，授权可随时撤销；工具面跟着授权走：实例只会看到自己真正能派发的工具（Leader spawn 的 worker 在拿到 `shell@workspace` 之前没有 shell）。
  有限下授、父级撤销级联、超时与未知结果都有恢复分类。
- **工作区策略**：`spawn` 可按需给实例共享目录、私有隔离目录或独立 Git worktree（各自分支）；
  终止实例时按记录回收，有未提交/未合并成果的目录永不自动删除，只报告原因。
- **多模型混合**：同一会话可混用三种线上协议（responses / anthropic / chat-completions）的真实实例，
  各自的模型、档位与原生上下文窗口独立配置；`spawn` 可指定 worker 用哪个 catalog 条目（键或模型名），
  一个团队可以混用条目。条目不认识时该工具调用直接报错；永远起不来的实例会被 park 并给出原因，不会拖住整个会话。
- **唯一权威状态**：每会话单 SQLite（WAL + `synchronous=FULL`）。崩溃恢复按持久化位置分类——
  已知结果复用、在途丢失诚实记账（`OUTCOME_UNKNOWN`）、**不猜测重放**。
- **权限门**：`approved_scope`（默认，bubblewrap 隔离，越权需批准）与 `full_auto`（仅用户可开，D-41
  主机 Shell）；一条批准绑定一个具体操作、其参数散列与权限 revision，只对那一次派发生效。
- **完成检查闸门**：`finish` 只接受诚实结论；目标结算前必须真实通过的是**你自己**配置里的 `[[checks]]`。
  项目文件目前还不能添加检查——合并加载器已实现，但没有任何入口调用它（见下文「配置与团队」）。
- **长上下文压缩**：按实际窗口占用触发，摘要保留原始要求、用户修订、验收与未决问题；
  原文经 `read_history` 仍可检索，压缩调用计入目标预算。
- **工具面**：文件读写/搜索（精确匹配编辑、SHA-256 版本校验、原子写入，进程内写锁串行化）、
  会话内持久 shell（`cd`/`export` 跨命令保留）、
  网页搜索与抓取、MCP（stdio 与 streamable HTTP）、Skills（`~/.agents/skills`）。
- **用户钩子（`[hooks]`）**：`notify` 把事件（`tool_call`/`team_action`/`run_*`）通知给你自己的程序；
  `pre_tool` 能在任何原生工具执行前拦截（exit 2 拒绝，stderr 作原因），坏钩子只记日志不卡团队。
- **隔离**：`approved_scope` 下实例 shell 走 `bubblewrap`；缺失时明确报错，不会静默退化成不隔离执行。

## 安装

要求：Linux（x86_64）+ `bubblewrap`；模型密钥从环境变量读，不写进配置。

### 安装最新版（推荐）

> **注意（2026-09-27）：** 最新发行版（`v0.1.2`）是**早期实现**，不是本文档所描述的产品——它的帮助里仍有本版本已移除的子命令。
> 在 v2 发行版发布之前，请按下文从源码构建，或阅读[安装指南](docs/INSTALL.md)（其中带有同样的说明）；
> `python3 review/install_check.py` 会重新对比两者。

仓库和 [发行版](https://github.com/seek-hope/TeamAgents/releases/latest) 已公开，无需登录 GitHub，
也无需 Rust 工具链。安装程序会自动选取最新版本、校验 SHA-256，并将两个程序安装到 `~/.local/bin`：

```bash
(
  set -eu
  installer="$(mktemp)"
  trap 'rm -f "$installer"' EXIT
  curl -fsSL https://raw.githubusercontent.com/seek-hope/TeamAgents/main/install.sh -o "$installer"
  sh "$installer"
)
export PATH="$HOME/.local/bin:$PATH"
```

将 `export PATH="$HOME/.local/bin:$PATH"` 加入 `~/.bashrc` 或 `~/.zshrc`，以后打开终端也能直接运行。
升级前退出 TeamAgents，再次执行即可；已有配置和会话会保留。
支持指定版本、自定义安装目录和本地发行包安装，详见 [安装指南](docs/INSTALL.md)。

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

安装 `bubblewrap`（Debian/Ubuntu：`sudo apt install bubblewrap`；其他发行版见安装指南），
然后在同一个终端中执行：

```bash
teamagents init                       # 创建内置最小配置；已有配置不会覆盖
export DEEPSEEK_API_KEY='你的模型密钥'  # 默认配置使用 DeepSeek；其他服务按配置中的 api_key_env 设置
teamagents doctor                    # 自检：配置 / 密钥 / 状态根 / 技能路径 / 隔离探针
teamagents --cwd /path/to/project     # 换成实际项目目录；省略 --cwd 则使用当前目录
```

`init` 遵循 XDG 配置目录；默认使用 DeepSeek Flash（1M 上下文），其他服务可编辑生成的 TOML。
从 v0.1.1 升级时，安装脚本会复制配置模板，可直接跳过 `init`。

进去以后直接说目标，例如：

```
把 /tmp/proj 里的测试修到全绿，改完说明每个文件为什么这么改。
```

Leader 会自己决定要不要组队、组几个人、谁干什么；需要你拍板时（越权限的命令、外网访问）
才会弹出批准请求。想看细节按 `Ctrl+N` 依次切换面板（实例 → 任务 → 拓扑），`Tab` 切换对话目标，`Ctrl+A` 看待批准。

脚本 / CI 用无头入口（`exec` 与 TUI 共用同一个 daemon；提示词写 `-` 就从 stdin 读，长指令可直接管道进来）：

```bash
teamagents exec "1+1 等于几？直接回答"                           # 提交一次输入，打印回复
teamagents exec --json --timeout 180 "把 /tmp/proj 的测试修绿"    # 机器可读摘要
teamagents exec --check "cargo test --offline" "改到测试全绿"     # 追加你自己的验收命令
git diff | teamagents exec -                                    # 提示词从 stdin 读
```

退出码：`0` 已结算，`1` 失败或未完成，`3` 有操作在等批准（无头运行没人能批，因此立刻返回而不是干等），
`124` 超过 `--timeout`，`2` 用法或环境错误（没有 daemon、没有模型 profile）。每个 `--check COMMAND`
在回合结束后按顺序在隔离 shell 里、在你的工作目录（`--cwd` 或当前目录）执行；第一条失败即停止，整个运行
判为失败。判定结果会打印出来、写入 `<state root>/verification.json`，并作为 `verification` 出现在
`--json` 报告里。

**会话的上限由你定。** 配置里的 `[limits]` 给每个 goal 设上限：`max_total_tokens` 会在用量到达上限前拒绝
新请求，`deadline_minutes` 会在超过截止时间后拒绝新请求；两者都不配置时，会话会一直跑到你手动停止为止（`doctor`
会把这件事直接说出来）。见 [docs/USER-GUIDE.md](docs/USER-GUIDE.md) §2.2。

**能力由你来发。** Leader 拿到它自己团队工具需要的那几项权限；它 spawn 出来的 worker 一项都没有，因此在
你授权之前，worker 只能用文件、网页和 skill 工具——编码任务通常需要的是共享工作目录的 shell：

```bash
teamagents authority                                    # 列出实例与授权（含撤销需要的 id）
teamagents authority grant --subject i-worker-1 --action shell --scope workspace
teamagents authority revoke --grant g-1a2b3c4d          # 撤销是终局的，派生授权一并撤销
```

每次派发都会重新校验授权，所以撤销也能拦住已经排队的工作，工具会在该实例下一次请求时从模型可见的工具面
消失。表面会拒绝那些「永远做不了任何事」的授权（动作不在词表内，或像 `shell@instance:i-worker-1` 这样没有
任何检查会问到的组合），并告诉你该用哪个资源范围。完整动作词表、退出码见
[docs/USER-GUIDE.md](docs/USER-GUIDE.md) §3.1。

## 常用参数与键位

| 用法 | 含义 |
|---|---|
| `--cwd DIR` | 以 DIR 为工作目录（默认当前目录）；只对这条命令启动的会话生效——已在运行的会话保留自己的工作目录，客户端会把它打印出来 |
| `--state-root PATH` | 指定状态根（默认 `$XDG_STATE_HOME/teamagents/v2`） |
| `--model KEY` | 选择模型目录键（默认 `leader_main`） |
| `--full-auto` | 用户显式开启全自动（仅用户可开；默认越权时请求批准）；只对这条命令启动的会话生效——已在运行的会话保留它启动时的模式，客户端会把这个模式打印出来而不是假装生效 |
| `init` / `doctor` / `daemon` / `exec` / `authority` / `approvals` / `instances` / `tasks` / `version` / `--help` | 准备配置与状态根 / 自检 / 单独运行 daemon（输出写入 `<state root>/daemon.log`）/ 无头输入 / 列出、发放与撤销能力 / 列出并决定待批准操作 / 暂停、恢复、终止并列出实例 / 列出并取消任务 / 版本 / 用法 |

TUI 键位（与屏幕底部提示一致；**刻意不使用 F 键**，因为部分键盘没有）：
`Enter` 发送、`Ctrl+J` 换行（终端会上报修饰键时 `Shift+Enter` 同样换行）、`Tab` 切换对话目标、
`Ctrl+N` 依次切换视图（对话 → 实例 → 任务 → 拓扑 → 回到对话）、`Ctrl+A` 跳到待批准、
`Esc` 返回、`Ctrl+C`/`Ctrl+D` 退出；
实例面板 `Enter` 设为对话目标、`p` 暂停、`r` 恢复、`t` 终止（需确认）；
任务面板 `c` 取消任务；批准面板 `a` 批准本次、`d` 拒绝。

## 配置与团队

- 会话读两份配置：你的 `$XDG_CONFIG_HOME/teamagents/config.toml`，以及它所处目录的
  `<cwd>/.teamagents/config.toml`。仓库那份**默认不起作用**，直到你在自己的配置里写
  `[permissions] trust_project = true`；之后它的 models、tools、skills 路径与 instruction 文件才会并入
  （同名时你的定义永远优先），而 `[permissions]`、hooks、checks、retention、limits 始终只来自你的配置。
  `doctor` 的 `project config` 行会报告合并结果（两处限制见 `docs/ACCEPTANCE.md`）。
- 模型 profile 用 `protocol = "responses" | "anthropic" | "openai" | "deepseek"` 选线上格式，
  `base_url` + `model` 决定实际接哪家；密钥只写环境变量名。
- 用户配置里的 `[[checks]]` 是"完成验收"的机器契约（只允许写在用户配置，项目文件不能定义）：
  目标声称完成时运行时会在隔离 shell 里执行这些命令，检查不过就打回修复，而不是让总结直接算成功。
- 组队由 Leader 在运行时按目标决定（`spawn`/`delegate`/`send`/`wait`），没有静态团队定义文件。
- 工具绑定即授权、批准语义、Skills/MCP、会话保留策略、故障处理：见
  [用户指南](docs/USER-GUIDE.md)。

## 现状与限制

- 只支持 Linux；成员 shell 隔离依赖 `bubblewrap`，缺失时明确报错而非降级为不隔离执行。
- 发行包只有 x86_64（musl）；暂无 Windows/macOS、分布式执行、远程成员协议、浏览器自动化。
- DESIGN §7 的四类协议族（chat-completions——`openai` 与 `chat/completions` 两个名字、DeepSeek 扩展、
  Anthropic、responses）都有本地假服务回归测试（`engine/tests/providers_fake.rs`），并由
  `python3 review/dogfood/protocols.py` 逐个对真实服务验收。环境中没有对应凭据的协议族会报告为**跳过**
  （`--strict` 会把跳过变成失败），因此实际能验收几类取决于运行机器的凭据（D-151）。
- 验收边界与证据见 [验收对照表](docs/ACCEPTANCE.md)，未做项与已知天花板在那里逐条列明。

英文版见 [README.md](README.md)；本文件是仓库里唯一允许出现中文的文件。

## 开发

开发先运行 `make check`；完整流程见 [开发与维护](docs/DEVELOPMENT.md)，仓库约定见 [AGENTS.md](AGENTS.md)。
缺陷、审查与评测证据记录在 `review/`，形式化验证证据在 `verification/`。
打 `v*` tag 会触发 `.github/workflows/release.yml` 自动构建并发布发行包（含 `SHA256SUMS`）。
