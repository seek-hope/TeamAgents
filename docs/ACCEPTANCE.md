# 验收对照表（A01–A36）

基准：[设计与验收基线](DESIGN.md) §12/§16。当前实现于 **2026-09-25** 核对。
✅ = 所列路径有自动化证据，不代表场景中的每项发布条件均已证明；🔶 = 部分覆盖/存在已知缺口；⚠ = 尚未实现。
逐阶段的落地过程（含迁移期记录）归档在 [R2 阶段记录](archive/STAGE-LOG-R2.zh-CN.md)。

`make check` 全绿（core 91 / engine 136 / tui 29）+ `make pty` 通过，是下面各条证据的公共前提。

## 验收矩阵 A01–A36（逐项证据）

| 编号 | 场景 | 证据 |
|---|---|---|
| A01 | 单 Leader 完成目标 | `v2_driver::end_to_end_shell_then_finish`；真实 DeepSeek 3 任务（2026-09-23） |
| A02 | A→B→C→A 通信 | `control::messages_flow_across_an_authorized_ring` |
| A03 | 有限下授与父撤销 | `control::grants_narrow_only_and_parent_revocation_cascades` |
| A04 | 排队动作遭遇撤权 | `revocation_blocks_queued_dispatch_until_reauthorized`、`dispatch_rechecks_permission_revision` |
| A05 | 读取其他实例历史 | `control::read_history_is_user_or_self_only`；daemon history 读面 |
| A06 | 消息应用事务前后重启 | `submit_input_applies_context_once_per_envelope`、`command_replay_returns_stored_receipt_and_rejects_conflict` |
| A07 | 永久启动错误 | `fail_request_closes_and_parks_without_losing_input`、`v2_spawn_failure::*` |
| A08 | 成功后消费前崩溃 | `v2_driver::tool_result_is_reused_after_crash_not_reexecuted` |
| A09 | 未知外部结果 | `control::unknown_outcome_parks_running_tasks_and_notifies` |
| A10 | 重复派发/GO | `jobs_runner::duplicate_go_starts_exactly_one_command` |
| A11 | daemon/runner 分别崩溃 | `jobs_runner::daemon_crash_reconnects_the_same_job_without_restart` |
| A12 | Shell 服务跨退出存活（D-41） | `jobs_runner::a_successful_commands_service_outlives_the_job`；真实 `approved_scope` 6 次批准执行 |
| A13 | 取消/超时/完成竞态 | `jobs_runner::cancel_*`、`v2_driver::user_cancel_stops_a_running_job` |
| A14 | bwrap 不可用 | `tools.rs` IsolationUnavailable；沙箱内实测分类失败（无主机回退） |
| A15 | 环境身份 | runner 持久化 pid+boot_id+start_ticks 并核验（jobs_runner） |
| A16 | 必需检查失败 | `v2_driver::required_checks_failure_repairs_then_passes`、`required_checks_exhausted_parks_the_goal_blocked` |
| A17 | 检查后产物变化 | `v2_driver::check_inputs_must_still_hold_at_completion` |
| A18 | 多实例用量预算 | `control::a_worker_shares_the_budget_of_the_goal_its_queue_serves` 等 |
| A19 | 半条流与失联 | `providers_fake::truncated_stream_before_output_is_transient`、`providers_stall::*` |
| A20 | 长上下文压缩后重启 | `control::compression_*`、`v2_driver::long_context_compacts_before_the_turn_and_survives_a_restart` |
| A21 | 用户直接调整 Worker | `submit_input` 单写上下文 + TUI 目标切换 |
| A22 | ALL/ANY 等待环与计时器 | `control::blocked_report_flags_dead_waits_not_cycles`、`a_due_timer_closes_the_wait` |
| A23 | 结果先到后注册等待 | `control::wait_for_an_arrived_result_is_satisfied_at_registration` |
| A24 | 重置后旧结果迟到 | `control::late_receipt_after_reset_lands_on_the_old_epoch_only` |
| A25 | MCP 批准/取消/未知 | `v2_mcp` 六项 |
| A26 | Skills 权限 | `v2_mcp::skill_call_without_the_binding_fails_honestly` |
| A27 | 异构供应商合作 | 真实：DeepSeek + Kimi 同会话双向真实消息（`review/tmp/r2-p5-a27/report.json`）；假服务：`v2_supervisor::heterogeneous_*` |
| A28 | 断连/慢客户端/重连 | `v2_daemon::handshake_checkpoint_command_and_goal_completion`、`reconnect_backfills_events_after_the_watermark` |
| A29 | 会话隔离/共享项目 | `control::begin_request_rejects_instances_of_other_sessions` |
| A30 | 制品与 DB 写入断点 | `control::artifact_staging_gc_and_publication_ordering` |
| A31 | 写入失败/磁盘满 | `control::disk_full_is_classified_at_the_submit_boundary`、`v2_driver::disk_full_stops_dispatch_reports_and_resumes_after_parking` |
| A32 | 超大历史测量 | `engine/examples/load_probe.rs` + `review/tmp/r2-p5-load/report.json` |
| A33 | 双 daemon/旧锁 | `v2_daemon::second_daemon_is_refused_and_shutdown_releases_the_lock` |
| A34 | schema 不兼容 | `core::v2::store::open_refuses_unstamped_foreign_and_wrong_version`、`open_migrates_the_previous_schema_version` |
| A35 | 目标截止时间 | `control::goal_deadline_refuses_new_requests_and_dispatches`、`v2_driver::goal_deadline_parks_the_instance` |
| A36 | 安装/init/doctor/清理/重开 | `cli::init_prepares_the_v2_root_and_doctor_verifies_it`；R28 清理 + 真实复验（`review/tmp/r28/`） |

## 升级须知（与更早发行版的差异）

- 更早发行版（≤ v0.1.2）的 `sessions/` 会话布局与旧配置**不被迁移**：`teamagents init` 准备当前状态根
  （默认 `$XDG_STATE_HOME/teamagents/v2`），`doctor` 只报告旧目录，旧的会话/偏好/缓存按清单清理
  （已完成，见 [决策记录](DECISIONS.md)）。
- 旧入口 `--team` / `--resume` / `--plain` / `validate` / `sessions prune` 不再存在：组队由 Leader 在
  运行时决定，恢复由 daemon 按事件水位重连，无头用法是 `teamagents exec`。传这些参数会得到明确的报错，
  不会静默忽略。
