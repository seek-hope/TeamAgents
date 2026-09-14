# TeamAgents 实现说明（Rust）

仓库现在**只保留 Rust 实现**：Python 原版在迁移完成后已从工作树移除（历史见 git，
`git show ba1caed:src/teamagents/...` 可查阅），TypeScript 层更早已全量移植并删除（D-17）。
本文中出现的 `src/teamagents/…`、`tests/test_*.py`、pytest 命令都是迁移期的历史引用。

## 架构划分

- **Rust（`core/`）= 权威核心**：`models`（spec 即数据，DP-1）、`storage`
  （SQLite WAL，DDL 与 Python 版逐字一致）、`control`（唯一事务入口
  submit：回执去重 → validate → reduce → persist → schedule）、`views`（信息权限）、
  `server`（方法分发；stdio 二进制与 engine 进程内调用共用同一份实现）。
- **Rust（`engine/`）= 产品层**：runtime 回合循环（线程模型）、ToolGateway/审批、
  成员后端（`ChatRunner` OpenAI 兼容工具循环、`CodexRunner` app-server、`ScriptedMember`）、
  工具执行器（文件沙箱/bwrap/SSRF 防护）、用户配置、会话清单与锁、CLI。
  二进制 `teamagents`：CLI + TUI 启动器 + `serve`（TUI 的无头会话服务，JSON-lines stdio）。
- **Rust（`tui/`）= 界面**：ratatui/crossterm 纯客户端；**Rust 原生设计**（D-20），
  不再追求与 main 的 Textual 界面逐像素一致。

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
| codex runner | codex.py | ✅ `engine::codex`（app-server JSON-RPC、thread 持久化、批准 park/decide、interrupt、`thread/read` reconcile、进程组清理） | fake-server + 合同测试 + 真实 CLI live 用例 |
| chat runner（替代 deepagents） | runners.py | ✅ `engine::chat`（工具循环 + renderView + 暂停/恢复 + provider→base_url + 按 `tool_bindings` 暴露 files/shell/web 执行工具 + Anthropic Messages 协议 + skills/指令注入 + 成员历史持久化） | chat_e2e 假 OpenAI harness + 真实 DeepSeek live 冒烟 |
| tools | tools.py + execution.py | ✅ `engine::tools`（文件工具沙箱；bwrap argv 与 execution.py 对齐、缺 bwrap 直接报错不降级、环境白名单；guardUrl 判定表与 Python 差分一致；web_fetch；AnySearch web_search；长输出落 artifacts 并可经 `/artifacts/` 前缀读回） | 单测（含真实 bwrap 运行）+ cli doctor |
| MCP 工具服务 | tools.py::build_bound_tools | ✅ `engine::bound` + `engine::mcp`（stdio 会话、`<service>_<tool>` 命名、tool_names 过滤、required/optional 语义、绑定即授权） | 真实 MCP stdio 服务器集成测试 |
| workspace 策略 | workspace.py | ✅ `engine::workspace`（shared/isolated/git_worktree、复用与回退规则、清理守卫、Leader 合并助手、会话删除守卫） | 单测（真实 git worktree 生命周期） |
| config | config.py | ✅ toml crate + XDG 路径 + catalog；项目配置合并（用户优先、`trust_project_tools`）；`[permissions] mode`；TeamSpec 支持 JSON/YAML | 单测 + CLI 用例 |
| sessions | sessions.py + session.py | ✅ 清单/flock 锁/归档/删除/open_session | worker 协议测试 + session_boot 锁用例 + TUI 冒烟 |
| CLI | cli.py | ✅ doctor/validate/sessions/version/--plain REPL/TUI 启动；doctor 实跑 bwrap 隔离探针与 codex schema 方法集合校验 | engine CLI 测试 |
| TUI | tui/（Textual ~1700 行） | ✅ ratatui/crossterm，**Rust 原生设计**（D-20：固定上下分区/滚动/胶囊/自适应列；D-18 的逐像素对齐已不再追求） | Rust 单元 + TestBackend 帧 + PTY 冒烟（含点击检查） |

**未移植/有意简化**：deepagents 图框架与 `general-purpose` 子代理（被 ChatRunner 工具循环
取代）；skills 恢复方案 §12.1 的"发现 + 按需读取"：`skill` 工具检索/读取注册根（`skills_paths`，只读），成员 `skills: [...]` 按名注入系统提示词（8KB/文件、32KB/成员上限，D-23；替代早期的全量注入简化）；
MCP 的 http/sse 传输（stdio 已实现）；TUI 以 Rust/终端习惯为准，不复刻 Textual 的组件外观
（D-20；Python 帧对比脚本保留为参考工具）。2026-09-13 全面审查后的其余保留差异（DENIED
重启记忆、Codex 审批 600s 上限、`kill` 退化路径、成员历史无上限、会话面板 size TTL、
web 前缀预授权、`/artifacts/` 可见性）见 `docs/DECISIONS.md` D-21；artifacts、成员历史持久化、
effort 归一化、doctor 探针、web 执行层 fail-closed 等已在 D-21 批次完成，不再是未移植项。

2026-09-13 追加审查修复了文件链接边界、Chat 回合检查点恢复、关闭与 shell 中断、
成员配置缓存以及 MCP 工具暴露，详见
[`追加修复台账`](../review/fix-notes-rust-followup-2026-09-13.md)。

## 既定决策

- 枚举线路格式与 Python StrEnum 完全一致（SCREAMING_SNAKE / snake_case），
  保证两版可读写同一份事件流与 DB。
- payload hash 用「键排序的规范化 JSON」的 sha256 前 32 位，与 Python
  `json.dumps(sort_keys=True)` 对齐。
- 移植顺序：control 按 action kind 逐个移植，每个以 `tests/test_t*.py`
  对应场景为 oracle；engine 侧同名场景见 `engine/tests/scenarios.rs`。

## 快速命令

```bash
# 构建（三个 crate）
for c in core engine tui; do (cd "$c" && cargo build); done

# 测试
cd core   && cargo test      # 38（17 unit + 21 integration）：权威核心
cd engine && cargo test      # 77（21 lib + 56 integration）：单测 + T1–T5/T9/T11–T13/T22 场景 +
                             #     取消/暂停 + 审批/全自动 + Codex 适配与合同 + worker 协议 +
                             #     CLI/doctor + bwrap + workspace + MCP + 崩溃恢复
cd tui    && cargo test      # 55（9 lib + 19 app + 27 render）：TUI 逻辑 + 帧冒烟 + 外壳断言
python3 tui/scripts/pty_smoke.py      # 真终端端到端冒烟（构建后）
python3 tui/scripts/pty_click_check.py  # 真终端点击命中检查（滚动后行命中）
cd engine && TEAMAGENTS_LIVE_CODEX=1 cargo test --test live_codex   # 真实 codex CLI 联调（可选）

# 运行
engine/target/debug/teamagents doctor          # 自检
engine/target/debug/teamagents                 # TUI（自动找 tui/target/*/teamagents-tui）
engine/target/debug/teamagents --plain         # 行模式 REPL
engine/target/debug/teamagents --team examples/team.yaml
```
