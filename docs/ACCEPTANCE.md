# 验收对照表（T1–T24）

> 表格主体是 **main 分支 Python 基准实现**的验收证据（`pytest tests/ -q` 确定性套件、
> `-m live` 真实服务套件）。**Rust 重构版（`reconstruct` 分支）的对照见文末**。

基准：方案 §17。**skip 不计入通过**：Anthropic / GLM / OpenAI 官方
三家在缺少有效密钥时显式 skip，相关用例已就绪，导出密钥即可跑。

| ID | 场景 | 证据 | 状态 |
|---|---|---|---|
| T1 | Leader 委派给 B 并汇总 | `test_t1_delegation.py::test_t1_delegation_and_summary`；真实模型版 `test_p3_live_session.py::test_live_leader_delegates_to_scripted_member` | ✅ |
| T2 | B/C 并行；慢任务不阻塞 Leader | `test_t2_parallel.py`（同步屏障证明同时开始 + 执行中补充） | ✅ |
| T3 | B/C 讨论；重复投递不重复注入；越界阻止 | `test_t3_discussion.py` | ✅ |
| T4 | 观察者只收到授权事件与载荷 | `test_t4_observer.py`（status scope 不泄露正文；观察≠发送/改团队） | ✅ |
| T5 | 共享空间发布/发现/权限/引用 | `test_t5_shared.py`（含分页与 supersedes） | ✅ |
| T6 | 信息隔离；新会话不继承 | `test_t6_isolation.py` | ✅ |
| T7 | 五家模型工具调用与续接；同队混用 | `test_p3_real_models.py`（DeepSeek ✅ / Kimi ✅ 真实通过；Anthropic / GLM / OpenAI 官方 skip）；混用在 `test_p3_live_session.py` 与 `test_p5_live_codex.py`（真实 Codex + DeepSeek） | 🔶 3/5 待密钥 |
| T8 | 恢复：杀进程后重建 | `test_p2_recovery.py`（含四类崩溃窗口） | ✅ |
| T9 | 单 Leader 可执行并继续对话 | `test_t1_delegation.py::test_scenario_without_delegation_single_leader`；真实版 `test_p3_live_session.py::test_live_single_leader_session_completes_goal` | ✅ |
| T10 | 自然语言组队；非法结构被拒绝 | 真实组队 `test_p3_live_session.py`；非法引用/未知模型/越权 `test_p1_guards.py` | ✅ |
| T11 | 动态变更：成员只能提议、Leader 应用、边界生效 | `test_p4_topology.py`（提案决策、drain 边界、换模型） | ✅ |
| T12 | 版本冲突不互相覆盖、不半应用 | `test_p4_topology.py::test_two_conflicting_patches_never_partially_apply`；`test_p1_guards.py::test_stale_patch_base_revision_conflicts` | ✅ |
| T13 | 移除成员：停止后移除、任务移交、成果保留 | `test_p4_topology.py::test_removed_member_hands_tasks_to_leader_and_keeps_audit` | ✅ |
| T14 | 执行中补充；仅相关成员按边界调整 | `test_p4_topology.py::test_mid_execution_supplement_reaches_running_leader`；`test_t2_parallel.py` | ✅ |
| T15 | 工具批准：自动/越界暂停/其他成员继续/拒绝 | `test_p3_deepagents_runner.py`（批准中断恢复、拒绝不执行、一次性批准消费） | ✅ |
| T16 | 全自动只能用户开启；仍守系统权限与 ACL | `test_p3_deepagents_runner.py::test_deepagents_full_auto_skips_approval`；`test_p1_guards.py`（模型不能开启） | ✅ |
| T17 | Codex 成员：任务/进度/结果/批准/取消/恢复映射 | `test_p5_codex_adapter.py`（生命周期、批准、取消、恢复、无团队工具）；真实 CLI `test_p5_live_codex.py` | ✅ |
| T18 | 工作目录 shared/isolated/worktree；脏输入不被忽略 | `test_p5_workspace.py`（含未合并拒绝清理、合并后清理） | ✅ |
| T19 | 工具生态：文件/Shell/搜索/抓取/MCP/Skills 真实任务 | `test_p3_web_tools.py`（AnySearch 实搜、抓取、SSRF）；MCP stdio 真实服务 `test_p3_tools_and_session.py`；Skills/AGENTS.md 注入同文件；Shell/文件 `test_p3_deepagents_runner.py` | ✅ |
| T20 | TUI：流式期间输入/导航/批准可用；窄屏、多行中文 | `test_p6_tui.py`；真实模型 + 真界面 `test_p6_tui_live.py` | ✅ |
| T21 | 崩溃去重：动作回执丢失仍只产生一次变更 | `test_p2_recovery.py`（重放去重、投递不重复注入） | ✅ |
| T22 | 资源与失败：限流/超时/成员失败/无人就绪/超限 | `test_p1_guards.py`（LIMIT_REACHED；依赖失败 → BLOCKED 并通知 Leader）；`test_p2_cancel_pause.py`；`test_p3_deepagents_runner.py`（步骤上限、成员失败被隔离） | ✅ |
| T23 | 权限执行：穿越/符号链接/Shell 越界/MCP 未授权 | `execution.py` 探针（`tests/test_p3_deepagents_runner.py::test_deepagents_shell_runs_isolated_inside_workdir` + P0 bwrap 越界记录 `docs/P0-findings.md`）；`test_p3_tools_and_session.py`（可选/必需 MCP 失败语义） | ✅ |
| T24 | 会话复用；同名新成员不继承旧身份 | `test_p2_recovery.py`、`test_t6_isolation.py`；`context_epoch` 机制见 `storage.py` | ✅ |

## 端到端示例（方案 §17 末尾）

| 示例 | 证据 |
|---|---|
| 项目修改并测试 | `examples/e2e_project_fix.py` 真实运行：Leader 委派 → worktree 成员受阻并如实上报 → Leader 亲验后修复合并 → `pytest 1 passed` |
| 联网调研并附来源 | `examples/e2e_research.py` 真实运行：AnySearch 搜索 + 抓取 → 结论与 2 个来源写入共享空间 → `goal_done` |
| 文件/数据整理并交付制品 | `examples/e2e_data_cleanup.py` 真实运行：清洗 `sales.csv` → `clean.csv`（42 行）+ `summary.md`（按 region 汇总）+ 制品引用 |

## 发布前必须补齐（未通过项）

1. **Anthropic / GLM / OpenAI 官方**：需要有效密钥后跑 `pytest -m live`（T7 剩余 3 家）。
2. 真实服务的限流/超时回归（T22 的供应商侧）建议在发布候选版本上再跑一轮。


---

## Rust 重构版（`reconstruct` 分支）对照

证据一律是可运行的：`cd core|engine|tui && cargo test`、`python3 tui/scripts/pty_smoke.py`、
`cd engine && TEAMAGENTS_LIVE_CODEX=1 cargo test --test live_codex`。
✅ = 有自动化证据；🔶 = 仅部分覆盖/仅人工实测；⚠ = 该能力未移植到 Rust 版。

| ID | Rust 版状态 | 证据 / 说明 |
|---|---|---|
| T1 | ✅ | `engine/tests/scenarios.rs::t1_delegation_and_summary_full_lifecycle` |
| T2 | ✅ | `scenarios.rs::t2_parallel_members_and_mid_run_supplement`（含执行中补充） |
| T3 | ✅ | `scenarios.rs::t3_channel_enforcement_and_exactly_once_delivery` |
| T4 | ✅ | `scenarios.rs::t4_observer_scoped_events_without_extra_rights` |
| T5 | ✅ | `scenarios.rs::t5_shared_space_permissions_and_discovery` |
| T6 | 🔶 | 会话按目录隔离、新会话不继承是核心语义（`core`）；Rust 侧无专属用例 |
| T7 | 🔶 | 真实 DeepSeek 单 Leader 回合实测（`--plain` 到 `goal_done`）；五家契约测试未移植 |
| T8 | 🔶 | 人工实测：`kill -9` 打断回合 → 重启 `reconcile` 收敛（requeue → COMPLETED）；无自动化用例 |
| T9 | ✅ | `scenarios.rs::t9_baseline_leader_alone_executes_and_keeps_talking` |
| T10 | 🔶 | 核心校验在内核（`core`）与 `teamagents validate`；自然语言组队的实时用例未移植 |
| T11 | 🔶 | 工具与内核在（`propose_team_change`/`apply_topology_patch`），Rust 无拓扑用例 |
| T12 | 🔶 | 版本冲突语义在 `core`（与 Python 同源）；Rust 无专属用例 |
| T13 | 🔶 | 同上 |
| T14 | ✅ | `scenarios.rs::t2_...`（supplement 到运行中的 Leader） |
| T15 | 🔶 | 审批门单测 + Codex 批准 park/decide 用例；ChatRunner 的批准暂停/恢复无专属用例 |
| T16 | ✅ | `scenarios.rs::full_auto_toggle_reaches_the_approval_gate`；仅 `actor=user` 可改模式（`core/src/control.rs` 校验） |
| T17 | ✅ | `engine/tests/codex_adapter.rs`（simple/approval/slow）+ 真实 CLI `live_codex.rs` |
| T18 | 🔶 | `shared`/`isolated` 已实现并人工实测（隔离成员文件落在自己 workspace）；`git_worktree` ⚠ 未移植（显式报错） |
| T19 | 🔶 | files/shell/web_search/web_fetch ✅（单测 + 真实 `echo` 回合）；MCP ⚠、Skills/AGENTS.md ⚠ 未移植 |
| T20 | ✅ | TUI 单测 + TestBackend 帧冒烟 + 真终端 PTY 冒烟；真实模型 + 真界面用例未移植 |
| T21 | 🔶 | 回执去重在内核（Python 同源测试）；Rust 侧靠 `worker_protocol.rs` 与人工重启验证 |
| T22 | 🔶 | 步骤上限/超时/取消在 runtime 内实现并有取消相关用例；限流类用例未移植 |
| T23 | 🔶 | `tools.rs::bwrap_argv_matches_python_and_runs_isolated`（真实 bwrap 运行）+ 越界路径单测；穿越/符号链接用例未系统移植 |
| T24 | 🔶 | 复用的 `context_epoch` 机制在核心；Rust 无专属用例 |

**Rust 版尚未移植（⚠）**：项目内配置、MCP 工具服务、Skills/AGENTS.md 注入、deepagents
子代理、`git_worktree` 工作目录策略、Anthropic 原生线协议。台账见 `docs/RECONSTRUCT.md`。
