# 验收对照表（T1–T24）

基准：方案 §17。证据一律是可运行的：`cd core|engine|tui && cargo test`、
`python3 tui/scripts/pty_smoke.py`、`python3 tui/scripts/pty_click_check.py`、
`cd engine && TEAMAGENTS_LIVE_CODEX=1 cargo test --test live_codex`。
✅ = 有自动化证据；🔶 = 仅部分覆盖/仅人工实测；⚠ = 该能力尚未实现。
**skip 不计入通过**：T7 中 Anthropic / GLM / OpenAI 官方三家缺有效密钥，导出密钥即可实跑。

| ID | 场景 | 状态 | 证据 / 说明 |
|---|---|---|---|
| T1 | Leader 委派给 B 并汇总 | ✅ | `engine/tests/scenarios.rs::t1_delegation_and_summary_full_lifecycle` |
| T2 | B/C 并行；慢任务不阻塞 Leader | ✅ | `scenarios.rs::t2_parallel_members_and_mid_run_supplement`（含执行中补充） |
| T3 | B/C 讨论；重复投递不重复注入；越界阻止 | ✅ | `scenarios.rs::t3_channel_enforcement_and_exactly_once_delivery` |
| T4 | 观察者只收到授权事件与载荷 | ✅ | `scenarios.rs::t4_observer_scoped_events_without_extra_rights` |
| T5 | 共享空间发布/发现/权限/引用 | ✅ | `scenarios.rs::t5_shared_space_permissions_and_discovery` |
| T6 | 信息隔离；新会话不继承 | 🔶 | 会话按目录隔离、新会话不继承是核心语义（`core`）；无专属用例 |
| T7 | 五家模型工具调用与续接；同队混用 | 🔶 | 真实 DeepSeek 单 Leader 回合实测（`--plain` 到 `goal_done`）；Anthropic 原生协议已实现（转换单测，缺密钥未实跑）；其余三家走 OpenAI 兼容路径未逐一实跑 |
| T8 | 恢复：杀进程后重建 | ✅ | `engine/tests/recovery.rs::t8_killed_turn_is_reconciled_and_stays_exactly_once`（kill -9 → 重启 requeue 重跑 → 取消收敛） |
| T9 | 单 Leader 可执行并继续对话 | ✅ | `scenarios.rs::t9_baseline_leader_alone_executes_and_keeps_talking` |
| T10 | 自然语言组队；非法结构被拒绝 | 🔶 | 核心校验在内核（`core`）与 `teamagents validate`；自然语言组队的实时用例未覆盖 |
| T11 | 动态变更：成员只能提议、Leader 应用、边界生效 | ✅ | `engine/tests/topology.rs::t11_member_proposal_is_leader_decision`（提案→Leader 应用→生效，审计保留提案人） |
| T12 | 版本冲突不互相覆盖、不半应用 | ✅ | `topology.rs::t12_conflicting_patches_never_partially_apply` |
| T13 | 移除成员：停止后移除、任务移交、成果保留 | ✅ | `topology.rs::t13_removed_member_hands_tasks_to_leader_and_keeps_results` |
| T14 | 执行中补充；仅相关成员按边界调整 | ✅ | `scenarios.rs::t2_...`（supplement 到运行中的 Leader） |
| T15 | 工具批准：自动/越界暂停/其他成员继续/拒绝 | ✅ | 审批门单测 + Codex 批准 park/decide 用例；ChatRunner 端到端：`engine/tests/chat_e2e.rs::once_approval_is_consumed_and_the_turn_completes`（once 执行后消费）、`expired_once_approval_requires_a_new_request`（EXPIRED 重请求）、`denied_approval_blocks_the_operation`；Codex 超时闭环 `codex_contract.rs::codex_approval_timeout_expires_the_row` |
| T16 | 全自动只能用户开启；仍守系统权限与 ACL | ✅ | `scenarios.rs::full_auto_toggle_reaches_the_approval_gate`；仅 `actor=user` 可改模式（`core/src/control.rs` 校验） |
| T17 | Codex 成员：任务/进度/结果/批准/取消/恢复映射 | ✅ | `engine/tests/codex_adapter.rs`（simple/approval/slow）+ 真实 CLI `live_codex.rs`；重启收敛 `codex_contract.rs::reconcile_reads_the_thread_history`、进程组清理 `closing_the_app_server_kills_its_process_group` |
| T18 | 工作目录 shared/isolated/worktree；脏输入不被忽略 | ✅ | `engine/src/workspace.rs` + 单测（worktree 生命周期/复用/合并/未合并拒绝清理/脏仓库回退 shared）；`tui` 会话删除守卫同源 |
| T19 | 工具生态：文件/Shell/搜索/抓取/MCP/Skills 真实任务 | ✅ | files/shell/web_search/web_fetch + MCP stdio（`engine/tests/mcp_tools.rs` 真实 MCP 服务器；`mcp_stdio.rs::server_environment_is_whitelisted` 环境白名单、`noisy_stderr_does_not_block_the_handshake`）+ MCP streamable HTTP（`engine/tests/mcp_http.rs`，D-25）+ Skills/AGENTS.md 注入（`session.rs` 单测）+ web 执行层 fail-closed（`tools_sandbox.rs::web_tools_are_fail_closed_and_ordered_by_member_binding`）+ 长输出 artifacts（`long_shell_output_is_stored_as_a_readable_artifact`）；旧式独立 SSE 传输 ⚠ |
| T20 | TUI：流式期间输入/导航/批准可用；窄屏、多行中文 | ✅ | TUI 单测 + TestBackend 帧冒烟 + 真终端 PTY 冒烟（冒烟 + 点击检查）；回归：`render_tests.rs::narrow_frames_render_without_panicking`、`tab_click_hits_the_tab_under_the_pointer`、`app_tests.rs::paste_fills_the_composer_without_submitting`、`panel_chords_never_fire_destructive_actions`；真实模型 + 真界面用例未覆盖 |
| T21 | 崩溃去重：动作回执丢失仍只产生一次变更 | ✅ | `recovery.rs::t8_...` 断言重放步骤不重复产生副作用（shared 条目仍为 1 条） |
| T22 | 资源与失败：限流/超时/成员失败/无人就绪/超限 | ✅ | `recovery.rs::t22_goal_turn_budget_is_enforced`（LIMIT_REACHED）+ 取消/暂停场景 + 模型步数上限 `chat_e2e.rs::model_step_limit_reports_limit_reached`（超限 → `limit_reached` + FAILED）+ 活动超时中断 `timeout_interrupts_the_member_before_further_side_effects`、崩溃不误报超时 `a_crashed_member_is_not_reported_as_a_timeout` |
| T23 | 权限执行：穿越/符号链接/Shell 越界/MCP 未授权 | 🔶 | `tools.rs::bwrap_argv_is_stable_and_runs_isolated`（真实 bwrap 运行）+ 越界路径单测 + `tools_sandbox.rs::guard_url_matches_the_blocked_range_table`（32 条阻断地址判定）+ `shell_survives_output_larger_than_the_pipe_buffer`；穿越/符号链接用例未系统覆盖 |
| T24 | 会话复用；同名新成员不继承旧身份 | 🔶 | 复用的 `context_epoch` 机制在核心；无专属用例 |

**尚未实现（⚠）**：成员私有子代理（方案中的 `general-purpose`）、旧式独立 SSE MCP 传输。
TUI 为 Rust 原生设计（D-20：固定分区、滚动、胶囊状态）。其余取舍见
`docs/DECISIONS.md`（D-17/D-19/D-20/D-21）。

## 更新记录

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
