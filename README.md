# TeamAgents

**你只和 Leader 说话，Leader 现场组队。** 运行在 Linux 终端上的团队式 Agent 产品：
你提出目标，Leader 按需招募工作实例、派发任务、协调协作、汇总结果；你负责看进度、在越权时点批准。

- **唯一权威状态**：每会话单 SQLite（WAL + `synchronous=FULL`），实例/任务/授权/预算/批准/回执与事件
  同事务提交；崩溃恢复按持久化位置分类，不猜测重放。
- **一个 daemon 拥有会话**：TUI 与 `exec` 都是它的薄客户端（Unix socket JSON 协议，断线按事件水位续读）。
- **受控协作**：`spawn`/`delegate`/`send`/`wait` 全部经控制平面授权与派发线性化点重查。
- **多供应商**：同一会话内可混用 DeepSeek（chat-completions）、Responses、Anthropic 三类协议的真实实例。

## 快速开始

```bash
teamagents init          # 写配置并准备 v2 状态根
export DEEPSEEK_API_KEY=...
teamagents doctor        # 配置 / 密钥 / 状态根 / 隔离探针
teamagents               # 打开 TUI（必要时自动拉起 daemon）
teamagents exec --json "用一句话自我介绍"    # 同一后端的无头输入
```

## 文档

| 文档 | 内容 |
|---|---|
| [使用指南](docs/USER-GUIDE.md) | 上手、配置、权限、Skills/MCP、恢复与清理 |
| [验收对照表](docs/ACCEPTANCE.md) | R2 阶段证据与 A01–A36 验收矩阵 |
| [开发与维护](docs/DEVELOPMENT.md) | 工具链、门禁、目录约定、复跑命令 |
| [重构方案](docs/TeamAgents-Agent-System-Rebuild-Plan.zh-CN.md) | R2-P0–P7、R01–R29、验收矩阵与完成定义 |
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

- **受控协作**：`spawn` / `delegate` / `send` / `wait` 都经控制平面授权，并在派发时重查权限版本；
  有限下授、父级撤销级联、超时与未知结果都有恢复分类。
- **工作区策略**：`spawn` 可按需给实例共享目录、私有隔离目录或独立 Git worktree（各自分支）；
  终止实例时按记录回收，有未提交/未合并成果的目录永不自动删除，只报告原因。
- **多模型混合**：同一会话可混用三种线上协议（responses / anthropic / chat-completions）的真实实例，
  各自的模型、档位与原生上下文窗口独立配置。
- **唯一权威状态**：每会话单 SQLite（WAL + `synchronous=FULL`）。崩溃恢复按持久化位置分类——
  已知结果复用、在途丢失诚实记账（`OUTCOME_UNKNOWN`）、**不猜测重放**。
- **权限门**：`approved_scope`（默认，bubblewrap 隔离，越权需批准）与 `full_auto`（仅用户可开，D-41
  主机 Shell）；批准绑定具体操作与参数散列，`once` 用后即失效。
- **完成检查闸门**：`finish` 只接受诚实结论；用户或项目预定义的必要检查必须真实通过。
- **长上下文压缩**：按实际窗口占用触发，摘要保留原始要求、用户修订、验收与未决问题；
  原文经 `read_history` 仍可检索，压缩调用计入目标预算。
- **工具面**：文件读写/搜索/原子多文件编辑、会话内持久 shell（`cd`/`export` 跨命令保留）、
  网页搜索与抓取、MCP（stdio 与 streamable HTTP）、Skills（`~/.agents/skills`）。
- **用户钩子（`[hooks]`）**：`notify` 把事件（`tool_call`/`team_action`/`run_*`）通知给你自己的程序；
  `pre_tool` 能在任何原生工具执行前拦截（exit 2 拒绝，stderr 作原因），坏钩子只记日志不卡团队。
- **隔离**：`approved_scope` 下实例 shell 走 `bubblewrap`；缺失时明确报错，不会静默退化成不隔离执行。

## 安装

要求：Linux（x86_64）+ `bubblewrap`；模型密钥从环境变量读，不写进配置。
`teamagents doctor` 还会探测本机 `codex` CLI（v1 的 Codex 执行成员才需要），未安装只报 WARN。

### 安装最新版（推荐）

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
teamagents doctor                    # 自检：配置 / 密钥 / 隔离 / Codex 可选能力 / 状态目录
teamagents --cwd /path/to/project     # 换成实际项目目录；省略 --cwd 则使用当前目录
```

`init` 遵循 XDG 配置目录；默认使用 DeepSeek Flash（1M 上下文），其他服务可编辑生成的 TOML。
安装旧版 v0.1.1 时，安装脚本会自动复制配置模板，此时跳过 `init`。

进去以后直接说目标，例如：

```
把 /tmp/proj 里的测试修到全绿，改完说明每个文件为什么这么改。
```

Leader 会自己决定要不要组队、组几个人、谁干什么；需要你拍板时（越权限的命令、外网访问）
才会弹出批准请求。想看细节按 `Tab` 切实例、`F3` 实例面板、`F4` 任务面板、`F5` 拓扑。

脚本 / CI 用无头入口（提示词可用 `-` 从 stdin 读；`exec` 与 TUI 共用同一个 daemon）：

```bash
teamagents exec "1+1 等于几？直接回答"                           # 提交一次输入，打印回复
teamagents exec --json --timeout 180 "把 /tmp/proj 的测试修绿"    # 机器可读摘要
teamagents exec --check "cargo test --offline" "改到测试全绿"     # 同一权限模式下追加产物检查
```

## 常用参数与键位

| 用法 | 含义 |
|---|---|
| `--cwd DIR` | 以 DIR 为工作目录（默认当前目录） |
| `--state-root PATH` | 指定 v2 状态根（默认 `$XDG_STATE_HOME/teamagents/v2`） |
| `--model KEY` | 选择模型目录键（默认 `leader_main`） |
| `--full-auto` | 用户显式开启全自动（仅用户可开；默认越权时请求批准） |
| `init` / `doctor` / `daemon` / `exec` / `version` / `--help` | 准备配置与状态根 / 自检 / 单独运行 daemon / 无头输入 / 版本 / 用法 |

TUI 键位（与屏幕底部提示一致）：`Enter` 发送、`Shift+Enter`/`Ctrl+J` 换行、`Tab` 切输入焦点与实例、
`F1` 对话、`F3` 实例、`F4` 任务、`F5` 拓扑、`F2` 批准队列、`Esc` 返回、`Ctrl+C`/`Ctrl+D` 退出；
实例面板 `Enter` 切换对话目标、`p` 暂停、`r` 恢复、`t` 终止（需确认）；
任务面板 `c` 取消任务；批准面板 `a` 批准本次、`d` 拒绝。

## 配置与团队

- 先读用户配置 `$XDG_CONFIG_HOME/teamagents/config.toml`，再合并项目配置
  `<cwd>/.teamagents/config.toml`（同名条以用户配置优先；项目工具绑定需
  `[permissions] trust_project_tools = true`）。
- 模型 profile 用 `protocol = "responses" | "anthropic" | "openai" | "deepseek"` 选线上格式，
  `base_url` + `model` 决定实际接哪家；密钥只写环境变量名。
- 组队由 Leader 在运行时按目标决定（`spawn`/`delegate`/`send`/`wait`），没有 TeamSpec 文件入口；
  v1 的 TeamSpec 样例已随该入口退役，放入 [`docs/archive/team.yaml`](docs/archive/team.yaml) 供追溯。
- 工具绑定即授权、批准语义、Skills/MCP、会话保留策略、故障处理：见
  [用户指南](docs/USER-GUIDE.md)。

## 现状与限制

- 只支持 Linux；成员 shell 隔离依赖 `bubblewrap`，缺失时明确报错而非降级为不隔离执行。
- 发行包只有 x86_64（musl）；暂无 Windows/macOS、分布式执行、远程成员协议、浏览器自动化。
- 三类线上协议（responses / anthropic / chat-completions）都已实现并有本地假服务回归；
  **五家真实模型服务的兼容性验收仍待具备相应凭据的环境**。
- 验收边界与证据见 [验收对照表](docs/ACCEPTANCE.md)，未做项与已知天花板在那里逐条列明。

## 开发

开发先运行 `make check`；完整流程见 [开发与维护](docs/DEVELOPMENT.md)，仓库约定见 [AGENTS.md](AGENTS.md)。
缺陷、审查与评测证据记录在 `review/`，形式化验证证据在 `verification/`。
打 `v*` tag 会触发 `.github/workflows/release.yml` 自动构建并发布发行包（含 `SHA256SUMS`）。
