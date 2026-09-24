# 协作与 TUI 修复审查（2026-09-12）

本轮已修复下表中的协作问题，并完成用户要求的上下布局和 Codex 风格 Leader 交互。修改前备份在 `.pre-fix-backup/collaboration-tui-20260912/`；本轮未修改用户正在使用的会话数据库。

## 协作问题与修复

| 问题 | 影响与修复 | 回归证据 |
|---|---|---|
| 成员工具提交绕过运行时通知 | Control 已持久化任务，但执行器未被唤醒；中途消息滞留。ToolGateway 复用 SessionRuntime.submit，提交后及时唤醒并投递 | `test_tool_dispatch_starts_worker_before_leader_finishes`：Leader 仍在等待时 worker 必须启动，消息立即到达 Leader 的中途收件队列；不依赖 settle 轮询唤醒 |
| 成员提案的批准绕过安全边界与版本校验 | 提案批准与 Leader 直接提交现在共用同一路径；保留提议人并记录决定人，过期版本拒绝 | `test_member_proposal_uses_boundary_and_records_decider`、`test_stale_proposal_is_rejected` |
| 新建成员附带通道导致 Leader 等待自身结束 | 单纯新增成员/通道授权可在控制事务内应用，Leader 可立即委派；撤销和配置修改仍等待边界 | `test_leader_can_create_delegate_and_cancel_without_waiting_for_itself` |
| 等待中的 patch 可能覆盖较新决定，或继续派发受影响成员 | 边界应用再次核对版本；冲突记为失败并通知 Leader；等待期间保留 DRAINING，应用或失败后恢复调度；调度读取最新 TeamSpec | `test_waiting_patch_conflict_cannot_overwrite_new_decision` 与原 T11–T13 测试 |
| BLOCKED 任务不能唤醒等待者，Leader 缺少取消工具入口 | BLOCKED 触发干预；Leader 的 cancel_task/cancel_run 复用已有控制动作和权限检查，普通成员仍无取消权限 | `test_blocked_task_wakes_waiter_for_intervention` 与 Leader 创建/取消回归 |
| 删除成员破坏其他收件人通道，残留观察主体；旧身份可被复用 | 仅删除目标成员的通道目标及观察主体；禁止复用已删除 ID、在同一 patch 删除重建同 ID，以及 update_agent 修改 ID；允许相同显示名使用新 ID | `test_remove_member_keeps_other_channel_targets_and_observer_scope`、身份回归组 |
| 恢复核对得到终态时只改 run 状态 | 已核实的终态复用 finalize，结算任务并通知请求者；结果不明保留为 OUTCOME_UNKNOWN，同时将任务 BLOCKED；已确认仍运行的成员保持 BUSY | `test_recovery_applies_completed_task_and_wakes_requester`、`test_recovery_keeps_confirmed_running_member_busy` 与原恢复测试 |
| 任务/回合终态与结果事件不在同一事务 | 终态、投递确认、完成事件及后续调度一起提交；事件写入失败时全部回滚，保留投递证据供重试 | `test_final_event_failure_rolls_back_task_run_and_ack` 与 F-C3 回归 |
| 原生取消只修改内存状态，仍可调用后续工具 | 在原生成员及私有子代理的模型/工具边界检查停止请求；当前工具返回前不宣称已停止，停止后不再启动后续模型/工具调用 | `test_native_cancel_waits_for_tool_and_prevents_next_side_effect` |

第一批六个复现测试在修复前全部失败，记录为 `review/tmp/collaboration-before.txt`；探针中的 FakeMember 字段名错误在记录该结果前已纠正。旧报告的 RT-06 批准残留与 TUI 任务取消入口在本轮开始前已经修复，本轮没有把它们再次算作新修复。

## TUI 交付

- 上区：原右区的团队、任务、共享空间、批准、会话、日志、设置。下区：原左区的 Leader 对话及输入。宽屏和窄屏均可访问两个分区。
- 参考 [Codex 源码与适配说明](../docs/archive/TUI-CODEX-REFERENCE.md)，使用现有 Textual/Rich 组件实现输入历史、相邻去重、草稿恢复、多行编辑、自适应高度、› 提示符、运行状态和快捷键提示。没有引入新依赖或替换运行时。
- 流式预览更新同一块区域，最终回复仅归档一次；Ctrl+R 不再重复追加历史。中文聊天在终端缩放后重新换行。
- Enter 发送；Shift+Enter/Ctrl+J 换行；↑↓ 调取历史；Ctrl+A/Ctrl+E 行首/行尾；Ctrl+G 打开批准；Esc 请求停止 Leader。会话切换清空并重建输入历史。
- 预览：[宽屏](teamagents-tui-wide.png)、[窄屏](teamagents-tui-narrow.png)。由 `review/tmp/render_tui.py` 使用假成员生成，无模型调用。

## 验证

- 修改前：130 passed, 12 deselected。
- 最终全量：**149 passed, 12 deselected，51.13 秒**；结果见 `review/tmp/full-collaboration-tui.txt`。新增 19 项确定性验收，12 项真实服务测试未运行。
- Textual 键盘、Markdown、流式预览、宽窄布局、会话管理、缩放回归已通过；预览已人工检查。
- doctor：依赖、用户配置、bubblewrap、Codex CLI 0.154.0、99 个协议方法校验通过。默认状态目录因当前沙箱只读而写入探针失败；使用仓库内独立状态目录后全部通过，记录在 `review/tmp/doctor-collaboration-tui-isolated.txt`。

## 验证边界

本轮运行确定性测试与本机协议自检，未执行 12 项真实模型/Codex 服务测试，不能据此宣告 P0–P7 全部发布验收完成。旧总报告中未在本表覆盖的安全、配置与外部适配发现，不视为已被本轮清零。

停止请求不回滚副作用；当前执行中的原生工具先返回，再在安全边界停止后续调用。无法及时确认停止时仍保留结果不明语义。TUI 流式预览最多保留最近 32,000 字符，持久化最终回复不因此截断。
