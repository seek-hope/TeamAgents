# 运行与取消审查（2026-09-17）

范围：`core/src`、`engine/src/runtime.rs`、`engine/src/gateway.rs`。基准为实现方案 §5–9、D-17 及当前验收表；本轮不变更产品架构，不使用真实模型凭据。

## 已修复的问题

1. **暂停会话会阻断活动成员的取消（高）**。调度循环在处理取消请求之前返回。活动成员收到 `cancel_run` 后仍可继续执行，直到会话恢复或活动时限耗尽。修复 `Runtime::start_ready_runs`，暂停状态仍检查取消请求，然后停止新调度。符合方案 §6.3、§9.3。
2. **迟到的回合结果会覆盖已取消状态（高）**。等待任务时取消可由 core 立即收敛为 CANCELLED，执行线程迟到的结果又可覆盖该终态，甚至使用旧完成申请把已取消任务改成 SUCCEEDED。`begin_run` 同样接受已终结的回合，存在调度快照与取消交错的启动窗口。修复 `Control::begin_run_inner`，仅允许 QUEUED/RUNNING 开始执行；修复 `finalize_run_inner`，已确认的 COMPLETED/FAILED/CANCELLED 不再接受迟到或重复的归档。OUTCOME_UNKNOWN 仍允许后端确认结果，未变更原恢复策略。符合方案 §5.3、§9.1–9.3。

## 证据与可复跑命令

```bash
cargo test --offline --manifest-path core/Cargo.toml --test engine late_completion_does_not_resurrect_a_cancelled_task_or_run -- --exact
cargo test --offline --manifest-path core/Cargo.toml --test engine cancelled_queued_run_cannot_begin -- --exact
cargo test --offline --manifest-path engine/Cargo.toml --test scenarios paused_session_still_cancels_an_active_member -- --exact
```

- 迟到完成回归：构造正在等子任务、已有完成申请的工作回合 → 用户取消工作 → core 确认 CANCELLED → 迟到的 COMPLETED 回调。修复前稳定失败，实际状态 `Completed`、预期 `Cancelled`；修复后回合与任务均保留 CANCELLED，事件表不新增伪完成事件。
- 过期启动回归：QUEUED 工作回合 → 用户取消任务 → 调度者拿旧快照调用 `begin_run`。修复前仍返回成功，修复后返回错误且事件表无新启动事件。
- 暂停取消回归：脚本成员已接到输入并开始 30 秒工作 → 暂停 → `cancel_run`，要求 2 秒内收到中断并收敛 CANCELLED，且会话保留 PAUSED。初版探针错误地用“脚本游标大于 0”判断启动；该游标只在 sleep 结束时推进，已改用 runner 保存的输入视图确认实际启动。随后临时移除暂停分支中的修复重新运行，正确地在“暂停不应阻断取消”断言失败；恢复修复后 0.09 秒通过。

## 回归结果

```bash
cargo test --offline --manifest-path core/Cargo.toml
cargo test --offline --manifest-path engine/Cargo.toml --test scenarios
git diff --check
```

core：22 库单测 + 37 集成测试全部通过；engine scenarios：10 项全部通过；空白检查通过。新增 3 项回归（core 2、engine 1）。主审负责最终三个 crate 的整合回归。

本轮证据为本地确定性状态机及真实运行时线程测试，不等于真实供应商恢复、取消或竞品水平验收。不涉及新依赖、外部服务或用户凭据。
