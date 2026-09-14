# 验收对照表（T1–T24）

> 表格主体是**迁移期 Python 基准实现**的验收证据（`pytest tests/ -q` 确定性套件、`-m live`
> 真实服务套件）；Python 源码已从仓库移除，这些用例作为历史证据保留。
> **当前实现的验收证据以 Rust 测试为准，对照见文末**。

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

## 当前 Rust 实现对照

2026-09-14 更新：树历史/检查点恢复、取消与迟到压缩、工具输出索引、MCP HTTP 协议、
Skills YAML 描述共 7 项缺陷已修复；另完成 `/model` 成员/供应商/模型/思考强度选择器和
slash 菜单末项显示修复；模型候选合并本机配置和供应商在线目录，后台获取不阻塞操作。
离线测试 **core 43 / engine 116 / tui 70** 通过，真终端冒烟、
点击及模型选择检查通过。测试名、复跑命令和边界见
[`修复台账`](../review/fix-notes-rust-updates-2026-09-14.md)。

2026-09-13 追加审查的 6 项修复与 10 项新增回归检查见
[`review/fix-notes-rust-followup-2026-09-13.md`](../review/fix-notes-rust-followup-2026-09-13.md)，
补充 T8/T21 的真实 Chat 崩溃恢复、T11 配置生效、T19 MCP 模型调用、T22 运行中 shell
中断及 T23 悬空符号链接证据。

证据一律是可运行的：`cd core|engine|tui && cargo test`、`python3 tui/scripts/pty_smoke.py`、
`python3 tui/scripts/pty_click_check.py`、`cd engine && TEAMAGENTS_LIVE_CODEX=1 cargo test --test live_codex`。
✅ = 有自动化证据；🔶 = 仅部分覆盖/仅人工实测；⚠ = 该能力未移植到 Rust 版。

| ID | Rust 版状态 | 证据 / 说明 |
|---|---|---|
| T1 | ✅ | `engine/tests/scenarios.rs::t1_delegation_and_summary_full_lifecycle` |
| T2 | ✅ | `scenarios.rs::t2_parallel_members_and_mid_run_supplement`（含执行中补充） |
| T3 | ✅ | `scenarios.rs::t3_channel_enforcement_and_exactly_once_delivery` |
| T4 | ✅ | `scenarios.rs::t4_observer_scoped_events_without_extra_rights` |
| T5 | ✅ | `scenarios.rs::t5_shared_space_permissions_and_discovery` |
| T6 | 🔶 | 会话按目录隔离、新会话不继承是核心语义（`core`）；Rust 侧无专属用例 |
| T7 | 🔶 | 真实 DeepSeek 单 Leader 回合实测（`--plain` 到 `goal_done`）；Anthropic 原生协议已实现（转换单测，缺密钥未实跑）；其余三家走 OpenAI 兼容路径未逐一实跑 |
| T8 | ✅ | `engine/tests/recovery.rs::t8_killed_turn_is_reconciled_and_stays_exactly_once`（kill -9 → 重启 requeue 重跑 → 取消收敛） |
| T9 | ✅ | `scenarios.rs::t9_baseline_leader_alone_executes_and_keeps_talking` |
| T10 | 🔶 | 核心校验在内核（`core`）与 `teamagents validate`；自然语言组队的实时用例未移植 |
| T11 | ✅ | `engine/tests/topology.rs::t11_member_proposal_is_leader_decision`（提案→Leader 应用→生效，审计保留提案人） |
| T12 | ✅ | `topology.rs::t12_conflicting_patches_never_partially_apply` |
| T13 | ✅ | `topology.rs::t13_removed_member_hands_tasks_to_leader_and_keeps_results` |
| T14 | ✅ | `scenarios.rs::t2_...`（supplement 到运行中的 Leader） |
| T15 | ✅ | 审批门单测 + Codex 批准 park/decide 用例；ChatRunner 端到端：`engine/tests/chat_e2e.rs::once_approval_is_consumed_and_the_turn_completes`（once 执行后消费）、`expired_once_approval_requires_a_new_request`（EXPIRED 重请求）、`denied_approval_blocks_the_operation`；Codex 超时闭环 `codex_contract.rs::codex_approval_timeout_expires_the_row` |
| T16 | ✅ | `scenarios.rs::full_auto_toggle_reaches_the_approval_gate`；仅 `actor=user` 可改模式（`core/src/control.rs` 校验） |
| T17 | ✅ | `engine/tests/codex_adapter.rs`（simple/approval/slow）+ 真实 CLI `live_codex.rs`；重启收敛 `codex_contract.rs::reconcile_reads_the_thread_history`、进程组清理 `closing_the_app_server_kills_its_process_group` |
| T18 | ✅ | `engine/src/workspace.rs` + 单测（worktree 生命周期/复用/合并/未合并拒绝清理/脏仓库回退 shared）；`tui` 会话删除守卫同源 |
| T19 | ✅ | files/shell/web_search/web_fetch + MCP stdio（`engine/tests/mcp_tools.rs` 真实 MCP 服务器；`mcp_stdio.rs::server_environment_is_whitelisted` 环境白名单、`noisy_stderr_does_not_block_the_handshake`）+ Skills/AGENTS.md 注入（`session.rs` 单测）+ web 执行层 fail-closed（`tools_sandbox.rs::web_tools_are_fail_closed_and_ordered_by_member_binding`）+ 长输出 artifacts（`long_shell_output_is_stored_as_a_readable_artifact`）；http/sse MCP ⚠ |
| T20 | ✅ | TUI 单测 + TestBackend 帧冒烟 + 真终端 PTY 冒烟（冒烟 + 点击检查）；新增回归：`render_tests.rs::narrow_frames_render_without_panicking`、`tab_click_hits_the_tab_under_the_pointer`、`app_tests.rs::paste_fills_the_composer_without_submitting`、`panel_chords_never_fire_destructive_actions`；真实模型 + 真界面用例未移植 |
| T21 | ✅ | `recovery.rs::t8_...` 断言重放步骤不重复产生副作用（shared 条目仍为 1 条） |
| T22 | ✅ | `recovery.rs::t22_goal_turn_budget_is_enforced`（LIMIT_REACHED）+ 取消/暂停场景 + 模型步数上限 `chat_e2e.rs::model_step_limit_reports_limit_reached`（超限 → `limit_reached` + FAILED）+ 活动超时中断 `timeout_interrupts_the_member_before_further_side_effects`、崩溃不误报超时 `a_crashed_member_is_not_reported_as_a_timeout` |
| T23 | 🔶 | `tools.rs::bwrap_argv_matches_python_and_runs_isolated`（真实 bwrap 运行）+ 越界路径单测 + `tools_sandbox.rs::guard_url_matches_the_python_guard_table`（32 条与 Python 判定表逐行差分）+ `shell_survives_output_larger_than_the_pipe_buffer`；穿越/符号链接用例未系统移植 |
| T24 | 🔶 | 复用的 `context_epoch` 机制在核心；Rust 无专属用例 |

**Rust 版尚未移植（⚠）**：deepagents 子代理（`general-purpose`）、旧式独立 SSE MCP 传输；
MCP Streamable HTTP（含 SSE 响应）已实现，见 D-25 与 `engine/tests/mcp_http.rs`。
TUI 为 Rust 原生设计（D-20），不再追求与 Textual 逐像素一致（不复刻滚动条字形与页脚溢出滚动）。
其余保留差异（部分覆盖而非缺失）见 `docs/DECISIONS.md` D-21。
台账与取舍见 `docs/RECONSTRUCT.md`、`docs/DECISIONS.md`（D-17/D-19/D-20/D-21）。
