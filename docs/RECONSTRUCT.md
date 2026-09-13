# TeamAgents Rust 重构（reconstruct 分支）

main 分支保留 Python 实现作为基准；本分支是**完整的 Rust 实现**（TypeScript 层已于
2026-09-13 全部移植进 Rust 并删除，见 docs/DECISIONS.md D-17）。

## 架构划分

- **Rust（`core/`）= 权威核心**：`models`（spec 即数据，DP-1）、`storage`
  （SQLite WAL，DDL 与 Python 版逐字一致）、`control`（唯一事务入口
  submit：回执去重 → validate → reduce → persist → schedule）、`views`（信息权限）、
  `server`（方法分发；stdio 二进制与 engine 进程内调用共用同一份实现）。
- **Rust（`engine/`）= 产品层**：runtime 回合循环（线程模型）、ToolGateway/审批、
  成员后端（`ChatRunner` OpenAI 兼容工具循环、`CodexRunner` app-server、`ScriptedMember`）、
  工具执行器（文件沙箱/bwrap/SSRF 防护）、用户配置、会话清单与锁、CLI。
  二进制 `teamagents`：CLI + TUI 启动器 + `serve`（TUI 的无头会话服务，JSON-lines stdio）。
- **Rust（`tui/`）= 界面**：ratatui/crossterm 纯客户端，逐像素复现 main 的 Textual 界面。

## 移植进度台账

| 模块 | Python 源 | 状态 | 验证 |
|---|---|---|---|
| models | models.py | ✅ 全量 | core 单测 |
| storage | storage.py | ✅ 全量 | engine 场景 |
| views | views.py | ✅ 全量（audience/push/scope + build_agent_view） | views + T4 场景 |
| control | control.py | ✅ 全量（validate/reduce/schedule/finalize/begin_run/wake_info） | 9 个 engine 场景 |
| server（方法分发） | core 原 stdio 服务 | ✅ 提取为 lib，stdio 与进程内共用 | core 单测 + CLI |
| runtime | runtime.py | ✅ `engine::runtime`（loop/reconcile/settle/mid-turn/cancel/timeout/步骤上限） | T1–T5/T9、取消/暂停场景 |
| gateway | agents.py + permissions.py | ✅ `engine::gateway`（ToolGateway/PermissionPolicy/ApprovalGate，含权限模式实时同步） | 单测 + full-auto 场景 |
| scripted member | agents.py::FakeMember | ✅ `engine::scripted`（含模板引用/barrier/取消） | 全部场景测试 |
| codex runner | codex.py | ✅ `engine::codex`（app-server JSON-RPC、thread 持久化、批准 park/decide、interrupt、reconcile） | fake-server 3 项 + 真实 CLI live 用例 |
| chat runner（替代 deepagents） | runners.py | ✅ `engine::chat`（工具循环 + renderView + 暂停/恢复 + provider→base_url + 按 `tool_bindings` 暴露 files/shell/web 执行工具） | 单测 + 真实 DeepSeek live 冒烟 |
| tools | tools.py + execution.py | ✅ `engine::tools`（文件工具沙箱；bwrap argv 与 execution.py 对齐、缺 bwrap 直接报错不降级、环境白名单；guardUrl + web_fetch；AnySearch web_search） | 单测（含真实 bwrap 运行）+ cli doctor |
| config | config.py | ✅ toml crate + XDG 路径 + catalog | 单测 |
| sessions | sessions.py + session.py | ✅ 清单/pid 锁/归档/删除/open_session | worker 协议测试 + TUI 冒烟 |
| CLI | cli.py | ✅ doctor/validate/sessions/version/--plain REPL/TUI 启动 | engine CLI 测试 |
| TUI | tui/（Textual ~1700 行） | ✅ ratatui/crossterm，D-16 复现基准不变 | Rust 单元 + TestBackend 帧 + PTY 冒烟 |

**未移植/有意简化**：deepagents 图框架本身（被 ChatRunner 取代）；TUI 的 zebra 条纹/鼠标
hover 与"会话内即席切换"；skills/memory 装配（`_skills_and_memory`）；MCP 工具服务与
`general-purpose` 子代理；anthropic 原生线协议（需 base_url 指向 OpenAI 兼容网关）。

## 既定决策

- 枚举线路格式与 Python StrEnum 完全一致（SCREAMING_SNAKE / snake_case），
  保证两版可读写同一份事件流与 DB。
- payload hash 用「键排序的规范化 JSON」的 sha256 前 32 位，与 Python
  `json.dumps(sort_keys=True)` 对齐。
- 移植顺序：control 按 action kind 逐个移植，每个以 `tests/test_t*.py`
  对应场景为 oracle；engine 侧同名场景见 `engine/tests/scenarios.rs`。

## 快速命令

```bash
cd core   && cargo test      # 权威核心（14 项）
cd engine && cargo test      # 引擎：单测 + T1–T5/T9 场景 + Codex 适配 + worker 协议 + CLI（24 项）
cd tui    && cargo test      # TUI 逻辑与帧冒烟（11 项）
python3 tui/scripts/pty_smoke.py      # 真终端端到端冒烟（构建后）
cd engine && TEAMAGENTS_LIVE_CODEX=1 cargo test --test live_codex   # 真实 codex CLI 联调
```
