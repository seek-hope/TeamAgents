# 目标完成提交竞态复核（2026-09-17）

依据：方案 §6.3、§6.4、DP-12；延续本轮对终态与迟到回调的修复。

## GC-01：完成申请到提交之间缺少重新验证（高，已修复）

`signal_done` 只在申请时检查任务、活动回合和批准；`finalize_run_inner` 正常结束时直接
写入 `done`。两次事务之间若 Leader 再派发任务，原完成申请仍能错误完成目标。
同样需要防止新用户输入使旧申请失效，以及旧回合的 `goal_id` 覆盖当前目标。

三个确定性回归在修复前均失败：

- `finalize_rechecks_work_added_after_signal_done`：`signal_done` 接受后，同一 Leader 回合
  又派发新任务，回合结束时 `goal_state` 仍变成 `done`，预期为 `active`。
- `new_user_input_invalidates_an_earlier_goal_completion_request`：完成声明后新用户要求到达，
  旧声明仍把目标完成；覆盖新输入未交付和已交付两种状态。
- `stale_goal_completion_cannot_overwrite_a_new_goal`：模拟旧申请恢复时，当前会话的
  `goal_id` 被改回旧目标，新的目标被丢失。

## 修复

- `finalize_run_inner` 提交目标完成时，在同一个 SQLite 事务中重新核对目标 ID 与完成阻塞项。
  申请失效只拒绝目标完成，仍正常结束当前回合、确认已交付输入，并留下进度事件。
- `UserMessage`/`UserSupplement` 在接受新要求时清除当前会话已有的目标完成声明，要求
  Leader 处理新输入后重新 `signal_done`。`Store::clear_goal_completion_requests` 只删目标
  声明，不删任务完成请求、不影响其他会话、无需数据库迁移。
- 已经由后端确认完成的当前回合，在重验阻塞项前于本事务内设置为完成，避免自己原有的
  `OUTCOME_UNKNOWN` 状态阻止合法恢复。新增
  `confirmed_recovery_can_commit_a_pending_goal_completion` 验证该路径。
- 新输入到达后重新 `signal_done` 的正常完成路径也在回归中覆盖。

## 验证

```bash
cargo test --offline --manifest-path core/Cargo.toml
cargo test --offline --manifest-path engine/Cargo.toml --test scenarios --test recovery --test chat_e2e
git diff --check -- core/src/control.rs core/src/storage.rs core/tests/engine.rs
```

core 全量 63 个通过（22 单测 + 41 集成）；补充“重新声明完成”分支后对应测试再次通过。
引擎相关测试 43 个通过（chat_e2e 31 + recovery 2 + scenarios 10）；差异空白检查通过。
未运行真实付费模型。
