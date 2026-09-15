# TeamAgents

运行在 Linux 终端上的**团队式 Agent 产品**：你只和 Leader 说话，Leader 按需组队、委派、
协调多个成员（内置成员 + 本机 Codex 执行成员）完成目标。
团队结构、通信权限、观察权限都是**运行时校验的数据**，不是提示词约定。

**全 Rust 实现**：`core/`（权威核心）、`engine/`（运行时/成员/CLI）、`tui/`（ratatui 界面）。
基准文档：`TeamAgents-Implementation-Plan.zh-CN.md`；设计决策 `docs/DECISIONS.md`；
审查与修复记录见 `review/`。

---

## 启动

### 1) 构建

```bash
for c in core engine tui; do (cd "$c" && cargo build); done
# 联网受限时加 --offline（依赖已在本机 cargo 缓存中）
```

要求：Linux + Rust 工具链（2026-09-15 用 Rust 1.95.0 验证；仓库尚未声明最低支持版本）；
`bubblewrap` 提供成员 shell 工具的隔离（缺失时明确报错，
不会退化成不隔离执行）；`codex` CLI 仅 Codex 执行成员需要。

### 2) 配置

```bash
mkdir -p ~/.config/teamagents
cp examples/config.toml ~/.config/teamagents/config.toml
export DEEPSEEK_API_KEY=...      # examples/config.toml 里 profile 引用的密钥
export ANYSEARCH_API_KEY=...     # 可选：AnySearch web_search；原生 web_fetch 无需此密钥
```

读取用户配置 `$XDG_CONFIG_HOME/teamagents/config.toml`，并合并项目配置
`<cwd>/.teamagents/config.toml`（同名条目用户定义优先；项目工具绑定需
`[permissions] trust_project_tools = true`）。MCP（stdio 与 streamable HTTP）与
Skills/指令文件均已支持：Skills 内容注入系统提示词。细节见 `docs/USER-GUIDE.md`。

### 3) 自检并进入界面

```bash
engine/target/debug/teamagents doctor       # 自检：依赖/配置/密钥/隔离/codex 协议（实跑 bwrap 与 codex schema 探针）/状态目录
engine/target/debug/teamagents              # TUI（默认；自动寻找 tui/target/*/teamagents-tui）
engine/target/debug/teamagents --plain      # 哑终端或脚本用行模式 REPL
```

常用参数与子命令：

| 用法 | 含义 |
|---|---|
| `--cwd DIR` | 以 DIR 为工作目录（默认当前目录；默认会话 id 由它派生） |
| `--resume <会话 id>` | 恢复会话（团队版本、待办、消息位置、成员线程、批准队列） |
| `--team SPEC` | 新会话使用指定 TeamSpec（JSON/YAML）；已有会话仍加载保存的团队定义 |
| `--full-auto` | 用户显式开启全自动（等价于 TUI 里 `Ctrl+F`） |
| `--plain` | 行模式 REPL（不发 TUI） |
| `doctor` / `validate SPEC` / `sessions [-v]` / `version` | 自检 / 校验 TeamSpec / 会话清单 / 版本 |

TUI 键位：`Enter` 发送、`Shift+Enter`/`Ctrl+J` 换行、`Ctrl+W`/`Alt+Backspace` 删词、
`Ctrl+←/→` 按词移动、`PgUp/PgDn`、`Ctrl+D`/`Ctrl+U`（或滚轮）滚动、`Ctrl+Home/End` 跳到最早/最新、
`Tab` 进管理面板（任意页签；面板内 `Esc`/`Tab` 返回输入框；面板动作只认无修饰字母键，
Ctrl/Alt 组合不会误触发）、
`Ctrl+T` 切面板、`Ctrl+G` 批准队列、`Ctrl+F` 全自动、`Ctrl+P` 暂停、`Ctrl+N` 回输入框、
`Esc` 输入框内=停止 Leader、`Ctrl+Q` 退出；
会话面板 `s`/`Enter` 切换、`n` 新建、`a` 归档、`d` 删除（连按两次确认）。
找不到 TUI 二进制时用 `TEAMAGENTS_TUI=/path/to/teamagents-tui` 指定。

### 4) 测试

```bash
# 以下命令均在仓库根目录执行
cargo test --offline --manifest-path core/Cargo.toml
cargo test --offline --manifest-path engine/Cargo.toml
cargo test --offline --manifest-path tui/Cargo.toml
python3 tui/scripts/pty_smoke.py                        # 真终端端到端冒烟（先构建）
python3 tui/scripts/pty_click_check.py                  # 真终端点击命中检查
TEAMAGENTS_LIVE_CODEX=1 cargo test --manifest-path engine/Cargo.toml --test live_codex -- --nocapture
```

当前离线结果与真实服务验收边界统一见 [验收对照表](docs/ACCEPTANCE.md)。普通 Chat 后端解析
OpenAI/Anthropic SSE 并向 TUI 提供增量预览；五家真实服务的兼容性仍待凭据环境下验收。

## 示例

```bash
# 单 Leader，一条命令验证端到端（真实模型）
printf '1+1 等于几？直接回答，然后 signal_done。\n' | \
  engine/target/debug/teamagents --plain --cwd /tmp/demo

# 带 Codex 执行成员的团队（先按下文配置 Codex 成员的 profile）
engine/target/debug/teamagents --team examples/team.yaml
```

`examples/config.toml` 给出模型 profile 与工具绑定样例；`examples/team.yaml` 展示混合团队结构。
其中 `codex_dev` 当前引用 DeepSeek 的 `leader_main`，不能据此保证 Codex 能运行：请将该成员
改为引用已配置、且本机 Codex 能使用的模型/provider，端点须支持 Responses API
（见 [Codex 配置说明](docs/USER-GUIDE.md#12-模型-profile)）。

## 文档地图

| 文档 | 内容 |
|---|---|
| `docs/DECISIONS.md` | 全部已确认决策（D-1..），含架构取舍与审查修复批次 |
| `docs/USER-GUIDE.md` | 配置、权限、恢复、故障处理、TeamSpec；TUI 布局与键位 |
| `docs/ACCEPTANCE.md` | T1–T24 验收对照；证据即 Rust 测试套件 |
| `docs/TUI-CODEX-REFERENCE.md` | TUI 交互约定与上游 Codex 参考 |
| `TeamAgents-Implementation-Plan.zh-CN.md` | 产品与实现基准 |
