# TeamAgents

**你只和 Leader 说话，Leader 现场组队。** 运行在 Linux 终端上的团队式 Agent 产品：
你提出目标，Leader 招募成员、派发任务、协调协作、汇总结果；你负责看进度、在越权时点批准。

- **结构由核心校验**：团队拓扑、通信权限、观察范围由核心在运行时校验并存入 SQLite。
- **混合团队**：内置成员可接 Responses / Anthropic / OpenAI 兼容（chat-completions）三类线上协议，
  也能直接拉起本机 `codex` CLI 当执行成员。
- **全程可见**：TUI 里有团队、任务、消息、共享空间、批准队列、计划、diff 审查与日志面板。
- **接得住中断**：回合被中断、任务卡住、目标没跑完，`--resume` 接着走。

## 怎么工作

```
你 ──目标（自然语言）──▶ Leader ──assign_task──▶ 成员 A / B / C（可并行）
                          │  ▲                        │
                          │  └────── send_message ─────┘  成员之间不能私聊，
                          │                             协作只走共享空间（Leader 可读全部条目）
                          ├── 观察者规则 / 批准请求 ──▶ 你（只在需要你决策时打扰你）
                          └── signal_done：Leader 声明目标达成，才算完成
```

- 你**总是**和 Leader 对话，不直接指挥成员；组不组队、组几个人由 Leader 按需求决定（一人成队合法）。
- 每个成员有自己的工作目录、工具绑定、模型与技能；越权操作变成**批准请求**，其他成员继续干活。
- 运行时事实（谁在做什么、任务卡在哪、回合为什么结束）都记在事件与任务状态里：可查、可恢复、可审计。

## 主要特性

- **自动组队与动态拓扑**：Leader 按自然语言目标组队；成员可 `propose_team_change`，
  改动经版本与权限校验后生效，无需用户逐步确认。
- **多模型混合**：同一团队里可以同时有 DeepSeek / Kimi / GLM / Anthropic / OpenAI 系模型与 Codex 执行成员。
- **共享空间与信息隔离**：`publish_shared` / `read_shared` 按读写权限；观察者只看被授权的对象与载荷；
  成员私有上下文不进 Leader 上下文，也不因此获得回信通道。
- **权限门**：`approved_scope`（默认，越界请求批准）与 `full_auto`（仅用户可开，TUI `Ctrl+F`）。
  批准绑定具体操作与参数：`once` 用后即失效，参数变了要重新批准。
- **执行前策略钩子**：`[hooks] pre_tool` 能在任何原生工具执行前拦下（exit 2 = 拒绝，stderr 作原因）；
  `[hooks] notify` 做异步事件通知。
- **断点恢复**：会话、任务、消息投递位置、成员线程、批准队列全部持久化；中断留下的
  "结果不明回合"可在界面里按 `c` 结清。
- **工具面**：文件读写/搜索/原子多文件编辑、持久 shell（`cd`/`export` 跨命令保留）、网页搜索与抓取、
  MCP（stdio 与 streamable HTTP）、Skills。
- **隔离**：成员 shell 走 `bubblewrap`；缺失时明确报错，不会静默退化成不隔离执行。

## 安装

要求：Linux（x86_64）+ `bubblewrap`；`codex` CLI 只有使用 Codex 执行成员时才需要；
模型密钥从环境变量读，不写进配置。

### 下载发行版

[Releases](https://github.com/seek-hope/TeamAgents/releases) 提供 x86_64 静态包
（musl 静态链接，不挑发行版 glibc）。仓库是私有的，用已登录的 `gh` 拉：

```bash
ver=0.1.1   # 换成要装的版本
gh release download "v$ver" --repo seek-hope/TeamAgents \
  --pattern "teamagents-$ver-x86_64-unknown-linux-musl.tar.gz" --pattern SHA256SUMS
sha256sum -c SHA256SUMS
tar -xzf "teamagents-$ver-x86_64-unknown-linux-musl.tar.gz"
install -Dm755 "teamagents-$ver-x86_64-unknown-linux-musl/"{teamagents,teamagents-tui} ~/.local/bin/
```

两个二进制装在同一目录（或同一 `PATH`）即可：`teamagents` 先找同目录的 `teamagents-tui`，再找 `PATH`。

### 从源码构建

```bash
for c in core engine tui; do (cd "$c" && cargo build); done   # 联网受限加 --offline
```

需要 Rust 工具链（2026-09-15 用 1.95.0 验证）。产物是 `engine/target/debug/teamagents`
与 `tui/target/debug/teamagents-tui`。

## 快速开始

```bash
mkdir -p ~/.config/teamagents && cp examples/config.toml ~/.config/teamagents/config.toml
export DEEPSEEK_API_KEY=...      # 配置里 profile 引用的密钥（发行包内示例叫 config.example.toml）
teamagents doctor                # 自检：依赖 / 配置 / 密钥 / 隔离 / codex 协议 / 状态目录
teamagents                       # 进入 TUI（源码构建则用 engine/target/debug/teamagents）
```

进去以后直接说目标，例如：

```
把 /tmp/proj 里的测试修到全绿，改完说明每个文件为什么这么改。
```

Leader 会自己决定要不要组队、组几个人、谁干什么；需要你拍板时（越权限的命令、外网访问、
团队变更）才会弹出批准请求。想看细节按 `Tab` 进面板。

行模式（哑终端 / 脚本；`-` 表示提示词从 stdin 读）：

```bash
printf '1+1 等于几？直接回答，然后 signal_done。\n' | teamagents --plain --cwd /tmp/demo
teamagents exec --json [--timeout SEC] [--check CMD] PROMPT|-    # 机器可读结果，脚本/CI 用
```

## 常用参数与键位

| 用法 | 含义 |
|---|---|
| `--cwd DIR` | 以 DIR 为工作目录（默认当前目录；默认会话 id 由它派生） |
| `--resume <会话 id>` | 恢复会话（团队版本、待办、消息位置、成员线程、批准队列） |
| `--team SPEC` | 新会话使用指定 TeamSpec（JSON/YAML）；已有会话仍加载保存的团队定义 |
| `--full-auto` | 用户显式开启全自动（等价 TUI `Ctrl+F`） |
| `--plain` | 行模式 REPL（不发 TUI） |
| `doctor` / `validate SPEC` / `sessions [-v]` / `version` | 自检 / 校验 TeamSpec / 会话清单 / 版本 |

TUI：`Enter` 发送、`Shift+Enter`/`Ctrl+J` 换行、`Esc` 停止 Leader、`Ctrl+Q` 退出；
`Tab` 进管理面板（`Ctrl+T` 切页签，面板内 `Esc`/`Tab` 回输入框）、`Ctrl+G` 批准队列、
`Ctrl+F` 全自动、`Ctrl+P` 暂停；
团队页签 `p` 看计划、`v` 看 diff，日志页签 `v` 看 diff，任务页签 `c` 结清无活动回合的任务。
完整键位与面板说明见 [用户指南](docs/USER-GUIDE.md)。

## 配置与团队

- 先读用户配置 `$XDG_CONFIG_HOME/teamagents/config.toml`，再合并项目配置
  `<cwd>/.teamagents/config.toml`（同名条以用户配置优先；项目工具绑定需
  `[permissions] trust_project_tools = true`）。
- 模型 profile 用 `protocol = "responses" | "anthropic" | "openai" | "deepseek"` 选线上格式，
  `base_url` + `model` 决定实际接哪家；密钥只写环境变量名。
- `examples/team.yaml` 是一个混合团队样例，可先 `teamagents validate examples/team.yaml`。
  其中的 `codex_dev` 只是占位（当前指向 DeepSeek profile，**不能**据此保证 Codex 成员能跑）：
  请改成你本机 Codex 真能用的 provider/模型，端点须支持 Responses API。
- 工具绑定即授权、批准语义、Skills/MCP、会话保留策略、故障处理：见
  [用户指南](docs/USER-GUIDE.md)。

## 现状与限制

- 只支持 Linux；成员 shell 隔离依赖 `bubblewrap`，缺失时明确报错而非降级为不隔离执行。
- 发行包只有 x86_64（musl）；暂无 Windows/macOS、分布式执行、远程成员协议、浏览器自动化。
- 三类线上协议（responses / anthropic / chat-completions）都已实现并有本地假服务回归；
  **五家真实模型服务的兼容性验收仍待具备相应凭据的环境**。
- 验收边界与证据见 [验收对照表](docs/ACCEPTANCE.md)，未做项与已知天花板在那里逐条列明。

## 文档与开发

| 文档 | 内容 |
|---|---|
| [docs/USER-GUIDE.md](docs/USER-GUIDE.md) | 配置、权限、团队定义、恢复、故障处理、TUI 布局与键位 |
| [docs/DECISIONS.md](docs/DECISIONS.md) | 全部已确认决策（D-1..）与架构取舍 |
| [docs/ACCEPTANCE.md](docs/ACCEPTANCE.md) | T1–T24 验收对照与证据 |
| [AGENTS.md](AGENTS.md) | 开发约定：架构速览、构建/测试命令、代码风格、审查规则 |
| [TeamAgents-Implementation-Plan.zh-CN.md](TeamAgents-Implementation-Plan.zh-CN.md) | 产品与实现基准 |

开发与测试命令在 [AGENTS.md](AGENTS.md)；缺陷与修复记录在 `review/`。
打 `v*` tag 会触发 `.github/workflows/release.yml` 自动构建并发布发行包（含 `SHA256SUMS`）。
