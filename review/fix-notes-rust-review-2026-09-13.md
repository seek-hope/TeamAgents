# Rust 重构全面审查 —— 修复台账（2026-09-13）

配套文档：`review/findings-rust-review-2026-09-13.md`（发现与证据）、本文（修复与残留）。

验收（修复后，全部实跑）：

| 套件 | 基线 | 修复后 |
|---|---|---|
| core `cargo test --offline` | 14 | **38**（17 unit + 21 integration） |
| engine `cargo test --offline` | 42 | **67**（21 lib + 46 integration） |
| tui `cargo test --offline` | 40 | **54**（9 lib + 18 app + 27 render；集成期补 Tab 可达性修复与测试） |
| PTY `tui/scripts/pty_smoke.py` / `pty_click_check.py` | — | 通过（含粘贴近 composer、滚动后点击命中、页签点击） |
| 真实模型冒烟（DeepSeek，`--plain`） | — | 通过（goal_done + Leader 回复） |

## core（写域 core/**）

| 编号 | 修复 | 关键测试 |
|---|---|---|
| C1/S3 | `submit`/`schedule`/`emit`/`begin_run`/`stop_timeout`/`finalize_run` 统一走 `in_tx`（BEGIN IMMEDIATE + 失败 ROLLBACK），错误上抛为错误 JSON；engine 侧不再吞（见 E1） | `emit_reports_store_errors_instead_of_swallowing_them`、`reduce_failure_rolls_back_and_the_refusal_replays`、`reduce_write_failure_rolls_back_the_shared_entry`、`malformed_params_are_errors_not_crashes` |
| C2/S4 | `load_team_spec` 返回 Result；旧 limits 键载入时丢弃（对齐 Python 存量兼容）；`stored_enum`、`get_action_receipt`、12 处 `filter_map(r.ok())` 不再静默丢行 | `legacy_limits_keys_are_dropped_on_load`、`missing_or_corrupt_spec_is_an_error_not_a_panic`、`unknown_stored_status_is_an_error_not_a_dropped_row` |
| C3 | `next_batch_no` 读改写回写、`last_applied_batch` 随 ack 推进 | `next_batch_no_advances_the_runtime_ledger`、`ack_advances_last_applied_batch`、`delivery_batch_ledger_round_trips_through_finalize` |
| C4 | `begin_run` 无条件发 `run_started`（payload 对齐 runtime.py:424-427，含 wake） | `begin_run_emits_run_started_with_the_wake_reason`、`begin_run_starts_the_task_before_announcing_the_run` |
| C5 | 新增 `expire_approval`（ONCE/PENDING→EXPIRED）与 `approval_find_run`（run+op_hash 下最新 PENDING/APPROVED_ONCE）两个 server 方法 | `expire_approval_only_touches_pending_and_approved_once`、`find_run_approval_returns_the_newest_usable_decision`、两个 endpoint 合同测试 |
| C6 | `wait_for_tasks`/`wake_info` 的 results 改为以 task_id 为键的对象 | `wait_for_tasks_reports_results_keyed_by_task_id`、`wake_info_reports_task_results_keyed_by_task_id` |
| C7 | `drop_pending_deliveries` 写 `payload_override={"dropped_reason":…}` | `drop_pending_deliveries_keeps_the_reason` |
| C8 | publish_shared 空内容拒绝；patch_id+空 operations 回退；read_shared 畸形分页参数报错 | `publish_shared_needs_nonempty_content_or_ref`、`apply_patch_with_empty_operations_uses_the_stored_operations`、`read_shared_rejects_malformed_paging_arguments` |
| C9 | `payload_hash` 分隔符对齐 Python `json.dumps` 默认 | `payload_hash_matches_python_json_dumps` |
| C10 | 删除 `PRAGMA synchronous=NORMAL`，回到与 Python 一致的默认 | 全量回归 |
| C11 | dispatch 错误面 / 锁竞争 / reduce 回滚用例补齐 | 见 C1 行 |

## engine 运行时层（runtime/chat/codex/gateway/scripted/core_client）

| 编号 | 修复 | 关键测试 |
|---|---|---|
| E3/S8 | 模型步数按会话 `limits.max_model_steps_per_turn` 计数（不再硬编码 200），超限设 `note="turn_limit"` → core 发 `limit_reached` | `model_step_limit_reports_limit_reached`（chat_e2e） |
| E2/S7 | 活动超时调 `request_interrupt`，超时后不再有幽灵副作用 | `timeout_interrupts_the_member_before_further_side_effects` |
| E4/S9 | once 批准：按 (run, op_hash) 命中未消费的 APPROVED_ONCE → 放行 → 执行后 `expire_approval` 消费；PENDING 复用行不重复插入；APPROVED_SESSION 放行不消费；DENIED 拒绝；EXPIRED 重新请求 | `once_approval_is_consumed_and_the_turn_completes`、`expired_once_approval_requires_a_new_request`、`denied_approval_blocks_the_operation` |
| E1 | engine 侧不再静默吞 core 错误：新增 `core_best_effort`，`set_run_status`/两次 `emit`/`stop_timeout`/`finalize_run`/`requeue_run`/`schedule` 失败打日志（集成收尾由主流程补） | 全量回归 |
| E5/F-5 | Codex reconcile 改用 `thread/read {includeTurns:true}`（原 `thread/status` 不存在） | `reconcile_reads_the_thread_history` |
| E6/F-7 | `xhigh→max` 按供应商归一化 + 被拒后改判 max 重试一次 | `effort_normalization_and_fallback`、单测分类表 |
| E7/F-8 | Codex 审批 600s 超时后 `expire_approval` 闭环；`resolve_approval` 失败打日志 | `codex_approval_timeout_expires_the_row` |
| E8/F-9 | `RecvTimeoutError::Disconnected` → "member runner crashed"（不再假报超时） | `a_crashed_member_is_not_reported_as_a_timeout` |
| E9/F-10 | Codex 子进程建独立进程组并杀组（经系统 `kill`，无新依赖） | `closing_the_app_server_kills_its_process_group` |
| E9b/F-11 | 重试只对 408/409/429/5xx 与传输错误，4xx 立即失败，末尾不再多睡，读 Retry-After | `retry_policy_only_retries_transient_errors` |
| E10/F-4 | 成员对话历史落盘 `<state>/sessions/<sid>/members/<agent>/chat_history.json`（tmp+rename），重启装载 | `conversation_history_survives_a_restart` |
| E11/F-6 | 新增假 OpenAI HTTP harness 与 codex 合同测试（共 12 项） | `engine/tests/chat_e2e.rs`、`engine/tests/codex_contract.rs` |
| 集成补 | `engine/src/gateway.rs::canonical_json` 分隔符对齐 Python `json.dumps`（operation_hash 跨版本一致） | `operation_hash_matches_python_json_dumps` |

## engine 工具/沙箱/配置层（tools/mcp/workspace/sessions/session/config/cli/worker）

| 编号 | 修复 | 关键测试 |
|---|---|---|
| N1/S5 | shell 输出改读线程 + 到点 kill：>64KiB 不再死锁，超时保留已读部分，截断按 char boundary | `shell_survives_output_larger_than_the_pipe_buffer` |
| E4 | 超限输出落 `/artifacts/exec-*.log` 并返回引用；成员 executor 支持 `/artifacts/` 前缀读写（防穿越） | `long_shell_output_is_stored_as_a_readable_artifact` |
| N2/S6 | MCP 子进程 stderr 改 inherit（不建管道） | `noisy_stderr_does_not_block_the_handshake` |
| N4 | MCP 环境白名单（HOME/LOGNAME/PATH/SHELL/TERM/USER）+ binding.env | `server_environment_is_whitelisted` |
| N3 | 会话锁改 Drop guard；失败可重试；半创建会话可修复重开 | `failed_open_releases_the_session_lock_and_can_be_retried` |
| N6 | `guard_url` 网段表镜像 Python `ipaddress`；IPv6/端口解析修复 | `guard_url_matches_the_python_guard_table`（32 条与 Python 差分逐行一致） |
| N7 | 会话锁改 `File::try_lock`（flock 语义，与 Python fcntl.flock 对应），抢占窗口消失，kill -9 自动回收 | `lock_is_held_by_the_live_holder_not_by_file_content`、`another_process_cannot_open_a_locked_session` |
| N8 | isolated `INPUTS.md` 不再被截断 | `isolated_workspace_inputs_note_is_not_truncated` |
| N9 | web 工具 fail-closed（未绑定报错）、绑定顺序确定、required 加载期校验 | `web_tools_are_fail_closed_and_ordered_by_member_binding` |
| N10 | doctor 增加 bwrap 实跑断言 + codex `app-server --help` + `generate-json-schema` 方法集合校验（D-3 兑现） | `doctor_probes_isolation_codex_and_config_errors` |
| N11 | `[permissions]` 类型/取值错误返回 Err（doctor 可见） | `parses_models_and_validates_the_permissions_section` |
| N12 | `dir_size_mb` 30s TTL 缓存（12.6ms → <1ms @2 万文件） | `dir_size_is_cached_between_calls` |

## TUI（写域 tui/**）

| 编号 | 修复 | 关键测试 |
|---|---|---|
| S1 | 浮层/toast 矩形夹取进 buffer（窄终端不再 panic）+ `main` panic hook 恢复终端 | `narrow_frames_render_without_panicking`、`settings_overlay_fits_narrow_frames` |
| S2 | 斜杠菜单 clamp min≤max，过窄不画 | 同上矩阵（slash 模式） |
| S10 | 几何单一来源：`Geometry{side_inner, rows, tabs_y}`，命中不再重算（含删除 `+1/-2` 与 `side.height-6`） | `clicking_a_scrolled_row_selects_the_row_under_the_pointer` |
| S11 | 面板动作键拒绝 CONTROL/ALT；Ctrl+D/U 在任何焦点下滚动 | `panel_chords_never_fire_destructive_actions`、`ctrl_d_and_ctrl_u_scroll_while_the_panel_is_focused` |
| T5 | 开启 bracketed paste（setup/teardown） | `paste_fills_the_composer_without_submitting` + PTY `?2004h` 断言 |
| T6 | 中文光标按显示宽度定位 | `composer_caret_follows_display_columns` |
| T7 | 滚轮按指针所在窗格（不再看 focus） | `wheel_targets_the_pane_under_the_pointer` |
| T8 | Ctrl+Home 由渲染侧真实回收到顶（删除 80 列估算） | `ctrl_home_reaches_the_oldest_entry_on_a_narrow_frame` |
| T9 | 语言下拉与值列对齐、不压 info 行 | `settings_dropdown_aligns_with_the_language_value_cell` |
| T10 | 未知斜杠命令不再发成消息（保留草稿 + system 提示） | `unknown_slash_commands_stay_out_of_the_chat` |
| T11 | `/settings` 描述不再提动效 | `settings_text_does_not_mention_animations` |
| 顺带 | CJK 泄漏扫描扩展（CJK 标点/假名/全角 + 菜单 + 浮层）；PTY 点击脚本覆盖滚动场景 | `ascii_frame_has_no_cjk_leaks`（扩展） |

## 已知残留（有意保留 / 待后续）

1. **DENIED 跨进程重启的重发**：engine 的 (run, op_hash)→approval 记忆表是进程内的；重启后对已拒绝操作的"新 id 重发"会重新请求批准（方向安全，需 core 透出最新 DENIED 才能完全对齐 Python 的原地拒绝）。
2. **Codex 审批 600s 有界等待**：Python 无界；保留 600s 并在超时后置 EXPIRED（`TEAMAGENTS_CODEX_APPROVAL_WAIT_S` 仅供测试覆盖）。
3. **Codex 进程组信号经系统 `kill` 二进制**（不加 libc 依赖）；`kill` 缺失时退化为只杀直接子进程。
4. **成员历史 JSON 无长度上限**（`ponytail:` 已注明，可改为按窗口裁剪）。
5. **TUI 会话面板 size_mb 30s TTL**：最长滞后 30s。
6. **web 工具策略层仍按 `web_` 前缀预授权**（执行层已 fail-closed；未绑定的调用会被 executor 拒绝，只是不弹批准）。
7. **`/artifacts/` 只挂进文件工具**；`ls/glob` 与 bwrap 内 shell 看不到（与 Python 基准一致）。
8. **MCP http/sse、deepagents `general-purpose` 子代理**：仍未移植（RECONSTRUCT 台账既有条目）。
