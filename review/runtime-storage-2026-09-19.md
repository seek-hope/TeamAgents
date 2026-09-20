# 运行时存储故障恢复与失败反馈

本批继续方案 §9/T8/T22 与 D-32/D-35，修复三项已经复现的错误：
成员返回后读取失败丢失原结果、启动视图失败不断生成替代回合，以及 CLI 把被拒绝的新输入误报为旧目标成功。
保持 Rust 三 crate 和 core 的 SQLite 单事务权威；新增诊断仅为进程内状态，没有修改数据库格式。

## 修复前证据

- `returned_chat_outcome_survives_a_state_read_failure_without_another_model_call`：
  成员成功返回后注入旧任务读取故障，原实现最终记录 FAILED，丢失原成功结果。
- `unreadable_member_view_keeps_one_queued_run_and_resumes_the_original_input`：
  共享游标不可读时，原实现先开始回合、再读取视图，失败归档后未消费输入不断生成新回合，
  消耗目标回合预算。两项原实现失败日志为
  `/tmp/teamagents-runtime-storage-before-20260919.log`。
- `exec_reports_rejected_input_instead_of_reusing_the_previous_completed_goal`：
  先完成旧目标，再用 SQLite trigger 拒绝新用户消息。原 `exec --resume` 仍输出 completed/0，
  并执行 `--check "touch CHECK_SHOULD_NOT_RUN"`。日志为
  `/tmp/teamagents-runtime-storage-cli-before-20260919.log`。

这些检查使用真实 `serve`/CLI 子进程、SQLite 和本地 HTTP 模型夹具，没有调用真实供应商。

## 当前行为

1. `Control::prepare_run` 在现有事务内完成回合开始、成员视图和唤醒信息读取。
   读取失败回滚状态、事件及投递裁剪；运行时保留同一个排队回合，按一秒间隔重试，
   尚未进入成员执行器时不产生失败归档或替代回合。
2. 成员返回后，运行时保留其原始 `TurnOutcome`，等待状态读取恢复后再核对取消并归档。
   读取错误不替换成功或失败结果，也不重新请求模型。归档事务失败继续沿用原有结果及投递确认重试。
3. 启动恢复的读取、重新排队和调度错误向上传播；恢复核对未成功时阻止新执行并重试。
   重试跳过当前正在执行或等待归档的回合。
4. `Runtime::errors()` 提供稳定的 `phase/run_id/agent_id/error`，可通过成功的运行时/worker
   状态回复读取。TUI 显示“等待存储恢复”，同一错误轮询不刷屏，全部清除时显示恢复信息。
5. `exec --json` 检查输入回执的 `ok`，受理成功后才启动运行时。
   输入拒绝、状态读取或运行时错误保留在结果的 `runtime_errors` 中，跳过本次 `--check`，
   返回 failed/1；已经发生的超时仍保持 124。原目标完成记录不会替代本次失败。

## 检查与结果

```bash
cargo test --offline --locked --manifest-path core/Cargo.toml --test stored_integrity
cargo test --offline --locked --manifest-path engine/Cargo.toml --test recovery
cargo test --offline --locked --manifest-path tui/Cargo.toml --test render_tests \
  storage_wait_is_visible_deduplicated_and_clears_after_recovery
make check
make pty
bash review/eval/check-runner.sh
```

新增一项 core 回归对 SQLite 原始值做全表快照，证明启动准备失败没有半提交，
故障解除后同一个回合正常开始且原输入保留。
新增三项 engine 回归分别覆盖原始成功/失败结果、同一个排队回合恢复，以及真实 CLI 拒绝输入；
检查无追加模型请求、原文件保留、输入正确消费、错误清除及验收命令未执行。
新增一项 TUI TestBackend 回归检查中英文实际帧、重复错误、恢复及再次故障。

| 检查 | 结果 | 日志 |
|---|---|---|
| `stored_integrity` | 11 passed | `/tmp/teamagents-runtime-storage-core-20260919.log` |
| `recovery` | 12 passed | `/tmp/teamagents-runtime-storage-recovery-final-20260919.log` |
| TUI 专项 | 1 passed | `/tmp/teamagents-runtime-storage-tui-20260919.log` |
| `make check` | core 138 / engine 352 / TUI 105；格式、严格 Clippy、卫生检查通过 | `/tmp/teamagents-runtime-storage-check-20260919.log` |
| `make pty` | 输入/恢复、滚动鼠标命中、工作区审查三项通过 | `/tmp/teamagents-runtime-storage-pty-20260919.log` |
| 评测 runner | 30 项无模型契约通过 | `/tmp/teamagents-runtime-storage-runner-20260919.log` |

engine 仍有三项显式 ignored；部分已有真实服务入口未启用时提前返回。
上述 595 项 Cargo 通过不能换算成真实服务验收数量。

## 边界与后续验证

- 这不是数据库自动修复。故障持续存在时仍无法推进；诊断不持久化，也不作为第二份执行事实。
  整体 `state` 无法读取时仍通过既有请求错误反馈，不保证每种数据库故障都能生成完整 TUI 状态帧。
- 成员返回后的等待结果保存在执行线程中；退出后的恢复仍依赖后端检查点或外部回合身份。
  本批没有新增“等待状态读取期间 SIGKILL”、取消与准备失败组合、恢复核对重新排队失败的专属故障测试；
  全量回归中的既有冷恢复测试不能代替这些新窗口的证据。
- `drain_mid_turn` 的读取/排空错误路径和 plain REPL 的回执处理没有在本批全面收口。
- 未新增真实供应商、远端工具、真实模型 TUI、陌生仓库任务或发行制品证据。
  T1–T24 仍为十八项有路径证据、六项部分覆盖，总体成熟度目标继续进行。

后续[运行中消息批次](mid-turn-storage-2026-09-19.md)已补齐 `drain_mid_turn` 交接/重试，
并增加实际 SIGKILL 后重新排队受阻的专属恢复检查。
后续[排队取消批次](queued-cancellation-2026-09-19.md)补齐准备视图失败时取消、旧取消请求重开，
以及结果等待状态读取期间正常关闭/实际 SIGKILL 的成功与失败结果恢复；后者在该批修复前已通过，
作为补充证据保留。
后续[plain 与终态恢复批次](repl-finalization-2026-09-19.md)处理 plain 的回执/存储等待反馈，
并补齐已返回终态在读取等待、归档重试时收到取消的恢复检查；其他未列出的故障组合保持。
