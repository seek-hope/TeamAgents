# TeamAgents TS+Rust 重构（reconstruct 分支）

main 分支保留 Python 实现作为基准；本分支逐步实现 TypeScript + Rust 版本。

## 架构划分

- **Rust（`core/`）= 权威核心**：`models`（spec 即数据，DP-1）、`storage`
  （SQLite WAL，DDL 与 Python 版逐字一致）、`control`（唯一事务入口
  submit：回执去重 → validate → reduce → persist → schedule）。
  对外是 stdio 换行分隔 JSON 服务（`teamagents-core` 二进制），不用
  napi/socket —— 好调试、语言无关。
- **TypeScript（`ts/`）= 编排与交互**：runtime 回合循环、runners、Codex
  协议适配、CLI、TUI。通过 `CoreClient`（stdio JSON）调用权威核心。
  零运行时依赖：`node:test` 跑测试，Node ≥22 原生执行 `.ts`。

## 移植进度台账

| 模块 | Python 源 | 状态 | 验证 |
|---|---|---|---|
| models | models.py | ✅ 全量 | core 单测 |
| storage | storage.py | ✅ 全量（含 finalize 支持函数） | engine 场景 |
| views | views.py | ✅ 全量（audience/push/scope + build_agent_view） | views + T4 场景 |
| control | control.py | ✅ 全量（validate/reduce/schedule/finalize/begin_run/wake_info） | 9 个 engine 场景 |
| runtime | runtime.py | ✅ TS `SessionRuntime`（loop/reconcile/settle/mid-turn/cancel） | T1/T2/T3/T4/T5/T9 TS 场景 |
| gateway | agents.py::ToolGateway + permissions.py | ✅ TS `ToolGateway`/`PermissionPolicy`/`ApprovalGate` | codex approval 测试 |
| scripted member | agents.py::FakeMember | ✅ `ScriptedMember` | 全部场景测试 |
| codex runner | codex.py | ✅ `CodexAppServer` + `CodexRunner`（thread 持久化、approval park/decide、interrupt、reconcile） | 3 个 fake-server 测试 |
| chat runner（替代 deepagents） | runners.py | ✅ `ChatRunner`：OpenAI 兼容工具循环 + renderView + 暂停/恢复语义 | 编译 + 场景间接覆盖 |
| tools | tools.py + execution.py | ✅ `workspaceExecutor`（文件工具沙箱、shell、bwrap、guardUrl、webFetch） | cli doctor |
| config | config.py | ✅ 迷你 TOML + 路径 + loadUserConfig | examples/config.toml 实测 |
| sessions | sessions.py + session.py | ✅ inventory/lock(pid 文件)/archive/delete + openSession | TUI 冒烟 |
| CLI | cli.py | ✅ doctor/validate/sessions/version/--plain REPL/TUI | doctor 实测 |
| TUI | tui/ (~1700 行 Textual) | ◐ `tui/app.ts`：零依赖 ANSI——标题/页签/面板/聊天/输入 + 持久历史 + 键位对齐；Textual 特有件（富表格 zebra、鼠标 hover）未复刻 | TUI 冒烟测试 |

**未移植/有意简化**：deepagents 图框架本身（被 ChatRunner 取代）；TUI 的 Textual 视觉细节；session 内切换会话（提示用 --resume）；skills/memory 装配（`_skills_and_memory`）；web_search 具体 provider 绑定（webFetch 已备）。需要时补。

## 既定决策

- 枚举线路格式与 Python StrEnum 完全一致（SCREAMING_SNAKE / snake_case），
  保证两版可读写同一份事件流与 DB。
- payload hash 用「键排序的规范化 JSON」的 sha256 前 32 位，与 Python
  `json.dumps(sort_keys=True)` 对齐。
- 移植顺序：control 按 action kind 逐个移植，每个以 `tests/test_t*.py`
  对应场景为 oracle；storage 操作随 control 需要增量补齐。

## 快速命令

```bash
cd core && cargo test            # Rust 核心测试
cd ts && node --test test/       # TS↔Rust 端到端冒烟
```
