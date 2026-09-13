# TeamAgents

运行在 Linux 终端上的**团队式 Agent 产品**：你只和 Leader 说话，Leader 按需组队、委派、
协调多个成员（内置 Deep Agents 成员 + 本机 Codex 执行成员）完成目标。
团队结构、通信权限、观察权限都是**运行时校验的数据**，不是提示词约定。

**本分支（`reconstruct`）= 全 Rust 实现**：`core/`（权威核心）、`engine/`（运行时/成员/CLI）、
`tui/`（ratatui 界面）。main 分支是 Python 基准实现，`src/teamagents/` 在本分支保留供对照。
基准文档：`TeamAgents-Implementation-Plan.zh-CN.md`；进度台账 `docs/RECONSTRUCT.md`；
设计决策 `docs/DECISIONS.md`。

---

## 启动（Rust 版）

### 1) 构建

```bash
for c in core engine tui; do (cd "$c" && cargo build); done
# 联网受限时加 --offline（依赖已在本机 cargo 缓存中）
```

要求：Linux + Rust 1.8x 工具链；`bubblewrap` 提供成员 shell 工具的隔离（缺失时明确报错，
不会退化成不隔离执行）；`codex` CLI 仅 Codex 执行成员需要。

### 2) 配置

```bash
mkdir -p ~/.config/teamagents
cp examples/config.toml ~/.config/teamagents/config.toml
export DEEPSEEK_API_KEY=...      # examples/config.toml 里 profile 引用的密钥
export ANYSEARCH_API_KEY=...     # 可选：web_search / web_fetch
```

只读用户配置 `$XDG_CONFIG_HOME/teamagents/config.toml`（本版**不读项目内配置**、不读 MCP 与
skills 配置，见 `docs/USER-GUIDE.md` §0）。

### 3) 自检并进入界面

```bash
engine/target/debug/teamagents doctor       # 核心/配置/密钥/隔离/codex/状态目录自检
engine/target/debug/teamagents              # TUI（默认；自动寻找 tui/target/*/teamagents-tui）
engine/target/debug/teamagents --plain      # 哑终端或脚本用行模式 REPL
```

常用参数与子命令：

| 用法 | 含义 |
|---|---|
| `--cwd DIR` | 以 DIR 为工作目录（默认当前目录；默认会话 id 由它派生） |
| `--resume <会话 id>` | 恢复会话（团队版本、待办、消息位置、成员线程、批准队列） |
| `--team SPEC` | 以指定 TeamSpec 开新会话（JSON 或 YAML） |
| `--full-auto` | 用户显式开启全自动（等价于 TUI 里 `Ctrl+F`） |
| `--plain` | 行模式 REPL（不发 TUI） |
| `doctor` / `validate SPEC` / `sessions [-v]` / `version` | 自检 / 校验 TeamSpec / 会话清单 / 版本 |

TUI 键位：`Enter` 发送、`Shift+Enter`/`Ctrl+J` 换行、`Ctrl+T` 切面板、`Ctrl+G` 批准队列、
`Ctrl+F` 全自动、`Ctrl+P` 暂停、`Ctrl+N` 回输入框、`Esc` 请求停止 Leader、`Ctrl+Q` 退出；
会话面板 `s`/`Enter` 切换、`n` 新建、`a` 归档、`d` 删除（连按两次确认）。
找不到 TUI 二进制时用 `TEAMAGENTS_TUI=/path/to/teamagents-tui` 指定。

### 4) 测试

```bash
cd core   && cargo test        # 14：权威核心（models/storage/control/views/server）
cd engine && cargo test        # 27：运行时 + T1–T5/T9 场景 + 取消/暂停 + 审批/全自动 +
                               #     Codex 适配 + worker 协议 + CLI + 沙箱 argv
cd tui    && cargo test        # 21：TUI 逻辑 + TestBackend 帧冒烟
python3 tui/scripts/pty_smoke.py                        # 真终端端到端冒烟（先构建）
cd engine && TEAMAGENTS_LIVE_CODEX=1 cargo test --test live_codex   # 真实 codex CLI 联调（可选）
```

## 示例

```bash
# 单 Leader，一条命令验证端到端（真实模型）
printf '1+1 等于几？直接回答，然后 signal_done。\n' | \
  engine/target/debug/teamagents --plain --cwd /tmp/demo

# 带 Codex 执行成员的团队（examples/team.yaml 是 YAML，engine 直接读）
engine/target/debug/teamagents --team examples/team.yaml
```

`examples/config.toml` 给出模型 profile 与工具绑定样例；`examples/e2e_*.py` 是 Python 版的
端到端示例（见下节）。

## 文档地图

| 文档 | 内容 | 适用版本 |
|---|---|---|
| `docs/RECONSTRUCT.md` | Rust 重构架构、移植台账、未移植项、快速命令 | Rust（本分支） |
| `docs/DECISIONS.md` | 全部已确认决策（D-1..D-17），含移植取舍 | 两版 |
| `docs/USER-GUIDE.md` | 配置、权限、恢复、故障处理、TeamSpec；§0 列出 Rust 版差异 | 两版（有标注） |
| `docs/ACCEPTANCE.md` | T1–T24 验收对照；末尾给出 Rust 版证据映射 | 两版（有标注） |
| `docs/STATUS.md` | P0–P7 阶段状态（Python 基准） | main（Python） |
| `TeamAgents-Implementation-Plan.zh-CN.md` | 产品与实现基准 | 两版 |

---

## Python 版（main 分支基准，本分支保留对照）

安装与使用（需要 Python ≥ 3.12、`uv`）：

```bash
uv venv && uv pip install -e .            # 或: pip install .
cp examples/config.toml ~/.config/teamagents/config.toml
.venv/bin/python -m teamagents doctor
.venv/bin/python -m teamagents --cwd /path/to/project     # Textual TUI
.venv/bin/python -m teamagents --plain                    # 行式 REPL
.venv/bin/python -m teamagents validate examples/team.yaml
```

两版能力已经对齐，Rust 版仍有以下已知差异（见 `docs/RECONSTRUCT.md` / `docs/DECISIONS.md`）：
MCP 仅支持 stdio 传输（http/sse 未实现）、Skills 走提示词注入而非虚拟文件系统、
没有 deepagents 的 `general-purpose` 子代理、TUI 不画 Textual 的滚动条字形。

```bash
.venv/bin/python -m pytest tests/ -q            # 确定性套件（脚本化成员）
.venv/bin/python -m pytest tests/ -q -m live    # 真实服务套件（需要密钥/本机 codex）
DEEPSEEK_API_KEY=... python examples/e2e_project_fix.py     # 项目修改并测试
DEEPSEEK_API_KEY=... ANYSEARCH_API_KEY=... python examples/e2e_research.py "问题"
```

更多：`docs/USER-GUIDE.md`（配置、权限、恢复、故障处理）、`docs/ACCEPTANCE.md`（验收对照表）。
