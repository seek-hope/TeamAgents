# 任务请求与迟到结算边界

本批继续方案 §5.2/§5.3/§6.2/§7/§9 和 D-32 的稳定性工作。
保留 `Control::submit` 与回合结算的 SQLite 单事务权威，使用本地协议夹具和实际数据库，
没有调用真实供应商服务，也没有新增架构偏离。

## 先复现的问题

- 成员执行任务 A 时提交同属自己的任务 B 的完成申请；用户随后取消 B，A 的迟到结算仍把
  B 从 `CANCELLED` 改为 `SUCCEEDED`。同成员在一个回合中完成其他待办是既有允许语义，不能一律禁止。
- 完成申请进入 `OUTCOME_UNKNOWN` 后，Leader 移除成员并接收任务；原回合迟到完成会把
  已移交任务改为成功，还会将已移除成员改回 `IDLE`。
- 同一 SQLite 含多个会话时，任务依赖、父任务、完成、等待、取消及动作的回合引用可读取其他会话；
  `begin_run`、超时停止、结算和若干恢复辅助接口也缺少一致的会话限定。
- 任务字段的错误类型被字符串转换或忽略，未知字段被静默接受。`description: true` 能创建任务。
- 已完成的回合仍可提交新的完成申请，覆盖原持久申请。
- 旧结果不明回合核对完成时，成员已经开始新回合，旧结算仍将成员从 `BUSY` 改为 `IDLE`。

修复前日志沿用前序批次命名：

- `/tmp/teamagents-task-boundaries-before-20260920.log`：初始六项，四项失败；
  明确观察到取消任务复活、已移交任务被完成及跨会话引用获准。
- `/tmp/teamagents-task-requests-before-20260920.log`：十项中的两项失败；
  错误类型请求和已结清回合的新完成申请均返回成功回执。
- `/tmp/teamagents-task-newer-run-before-20260920.log`：旧回合结算后实得 `Idle`，预期 `Busy`。

## 修复与保留的语义

1. `assign_task`、`complete_task`、`wait_for_tasks`、`cancel_task`、`cancel_run`
   使用严格 serde 请求类型，拒绝未知字段、错误类型和显式 null 冒充省略。
   `assign_task` 保留显式 `task_id`、`parent_task_id`，`wait_for_tasks` 保留合法空数组。
   实际模型工具 schema 同步声明字段与 `additionalProperties: false`。
2. Store 提供按 session 与主键共同读取任务/回合的方法；Control 的任务/回合查询及任务移交使用会话限定。
   恢复辅助 JSON 接口在读写前复核回合归属，批准查询过滤会话，插入批准复核已存在回合的成员归属。
3. 开始回合时复核附属任务仍属于当前会话、承接者匹配且为 `PENDING`/`RUNNING`；
   拒绝时事务回滚，不留下半启动状态。已完成、失败或取消的回合拒绝新动作，
   原动作的相同请求仍可重放持久回执；等待、排队和结果不明状态保留既有恢复路径。
4. 正常结束时重新检查完成申请的当前承接者、成员存在性、任务状态及成果引用。
   已取消、已结清、已移交及不符合条件的 BLOCKED 任务不被改为成功；
   原申请作为审计保留，以 `run_progress` 记录失效原因，不生成 `task_completed`。
   同成员完成另一合法待办的既有路径继续通过。
5. 原 `OUTCOME_UNKNOWN` 回合仍可用已确认结果结算其自身、仍归原成员的 BLOCKED 任务。
   此例外不适用于其他任务或终态任务。已结清回合重复归档保持幂等；
   已移除成员不复活，存在同成员其他排队/执行/等待回合时不覆盖成员当前状态。
6. 投递确认必须属于当前会话和执行成员。结算读取或审计写入失败向上传播，
   任务、回合、成员及投递状态在同一事务中回滚；解除故障后可重试原结算。

## 可复跑检查

```bash
cargo test --offline --locked --manifest-path core/Cargo.toml --test task_boundaries
cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e \
  task_requests_can_be_corrected_through_model_tools_and_survive_reopen
cargo test --offline --locked --manifest-path engine/Cargo.toml --lib \
  prompt_overhead_stays_lean -- --nocapture
make check
make pty
```

新增 core 十五项均在 `core/tests/task_boundaries.rs`：

| 检查范围 | 测试名 |
|---|---|
| 取消与合法同成员待办 | `task_completion_cannot_resurrect_a_cancelled_sibling`、`task_completion_preserves_successful_sibling_delivery` |
| 移交、结果核对与新回合状态 | `task_completion_rechecks_ownership_after_member_removal`、`task_completion_can_reconcile_its_original_unknown_run`、`task_completion_reconciliation_preserves_the_members_newer_run_status` |
| 跨会话动作与回合启动 | `task_actions_reject_foreign_session_references`、`task_run_lifecycle_rejects_foreign_session_without_side_effects`、`task_run_cannot_start_with_a_settled_reassigned_or_foreign_task` |
| 终态保护、移交与旧等待 | `task_completion_keeps_terminal_results_and_rejects_new_requests_from_settled_runs`、`task_handover_and_stale_waits_stay_in_their_session` |
| 故障回滚、严格请求与投递确认 | `task_finalization_read_and_audit_failures_roll_back_and_can_be_retried`、`task_requests_refuse_malformed_fields_without_changing_work`、`task_finalization_refuses_acknowledging_another_sessions_delivery` |
| 恢复接口与数据库重开 | `task_server_endpoints_cannot_read_or_update_a_foreign_run`、`task_refusals_and_stale_completion_survive_database_reopen` |

新增 engine 一项 `chat_e2e::task_requests_can_be_corrected_through_model_tools_and_survive_reopen`：
通过生产 `open_session`/Runtime/ChatRunner，在普通和全自动模式分别执行错误 assign → 修正
→ 错误 complete → 实际写文件 → 合法完成。检查下一次模型请求收到对应失败/成功回执、
严格工具 schema、精确文件内容、唯一完成事件及重开后不请求模型。

定向十五项与提示开销检查通过。系统提示 485 字符、工具 schema 11990 字符；
此前 12222 字符触发原有 12000 上限，已压缩描述，未扩大预算阈值。

全量 `make check` 通过格式、严格 Clippy、三个 crate 回归及仓库卫生检查：
**core 115 / engine 346 / TUI 104**；engine 另有三项显式 ignored，
真实 Codex 开关未启用。完整日志：
`/tmp/teamagents-task-boundary-final-check-20260920.log`。

`make pty` 三项通过：输入/恢复冒烟、滚动后鼠标命中、工作区审查与恢复基线。
日志：`/tmp/teamagents-task-boundary-final-pty-20260920.log`。

## 验收边界

- 本批数据库重开与会话对象重建不是新增 SIGKILL 或真实模型证据；
  既有 Chat/Codex SIGKILL 用例随全量回归通过。
- 任务引用与状态检查不代替成果内容或验收命令验证，也不提供通用外部副作用 exactly-once。
- 其他动作和 JSON 辅助协议尚未全面严格化，部分旧数据库行仍有宽松 JSON 解码；
  本批不宣称解决所有损坏状态或跨存储事务问题。
- T1–T24 仍为十八项有路径证据、六项部分覆盖。真实供应商矩阵、工具生态与真实模型 TUI
  等未验证项继续保留，不由新增本地测试数量折算完成度。
