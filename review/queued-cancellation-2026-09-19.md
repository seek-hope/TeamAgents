# 排队取消、输入范围与结果等待期间退出

本批继续方案 §6.3、§7、§9 和 D-32/D-35，收口排队回合取消及已返回结果等待读取恢复的边界。
生产改动位于 `core/src/control.rs`，沿用 SQLite 单事务权威、原数据库格式与后端停止确认规则。
模型使用本地 HTTP 协议夹具；真实 `serve` 子进程和 SIGKILL 不等于真实供应商验收。

## 修复前证据

1. `CancelRun` 对排队回合只设置取消请求。成员视图读取失败使 `prepare_run` 持续回滚，
   回合无法开始，也就无法进入执行器处理取消。生产 `serve` 回归先失败：用户输入已受理、
   `prepare` 诊断可见，提交取消后原回合仍为 QUEUED。
2. `CancelTask` 原先直接取消关联排队回合，却留下该任务的待投递输入；同次调度又以旧任务通知
   生成没有 `task_id` 的聊天回合。扩展已有 `cancel_task_cancels_queued_run` 后先失败。

复现日志：

- `/tmp/teamagents-queued-cancel-before-core-20260919.log`
- `/tmp/teamagents-queued-cancel-before-integrity-20260919.log`
- `/tmp/teamagents-queued-cancel-before-engine-20260919.log`

初步修复曾将任务所在排队回合的全部输入丢弃。新检查
`cancelling_one_queued_task_preserves_other_messages_and_assignments` 先证实同回合普通消息也被误丢，
日志 `/tmp/teamagents-cancel-input-scope-before-20260919.log`。最终实现按下述任务范围处置输入，
不把这一中间实现的问题冒充既有发行版缺陷。

## 最终行为

- 无外部回合 ID 的 QUEUED 可在核心事务中直接取消，不依赖成员输入视图，也不启动成员执行器。
  同事务取消仍归该成员且可取消的任务、过期待批准、记录事件；整回合取消给其尚未消费输入
  记录 `dropped_reason`，不伪记为模型已消费。
- 取消单个任务仅丢弃该承接者待投递的 `task_ready`，关联使用事件的 `event_task_id`。
  关联排队回合结清后，普通消息和其他任务继续调度；目标任务只是另一回合的附带通知时，
  原回合继续，只移除目标任务通知。没有其他输入时不会因旧任务通知生成替代聊天回合。
- 输入、批准、任务或事件写入失败使整次取消回滚。拒绝回执按原动作去重规则保留，
  故障排除后用新动作 ID 重试；成功回执重放不重复落事件或改动状态。
- 调度兼容旧版留下的 QUEUED `cancel_requested`。仍带外部回合 ID 时仅请求取消，
  不提前声明外部执行已停止。已移除成员不因结清旧回合而复活。
- 挂起等待回合沿用原投递确认策略；本批未将全部等待/运行中取消输入语义重新设计。

## 新增与扩展检查

| 检查 | 证明范围 |
|---|---|
| `stored_integrity::cancelling_a_queued_run_does_not_require_its_view_or_replay_its_inputs` | 活动态/暂停态、坏成员视图下取消，输入留失效原因但消费批次不推进，无重复回合/开始事件，成功回执重放无额外变更 |
| `stored_integrity::queued_cancellation_failure_keeps_the_run_approvals_and_input_together` | SQLite trigger 拒绝输入失效；无任务回合、带任务回合和任务取消三种路径，除拒绝回执外原始全表一致，恢复后任务/回合/批准一起结清 |
| `stored_integrity::scheduling_converges_an_older_queued_cancel_request_without_preparing_its_view` | 旧取消标记、坏视图和待批准并存，调度可结清，重复调度无重复事件 |
| `engine::cancelling_one_queued_task_preserves_other_messages_and_assignments` | 同一排队回合混入普通消息及第二任务，取消目标后实际成员输入保留其余内容，目标不重播，任务取消事件恰好一次 |
| `engine::cancelling_a_second_task_removes_only_its_wake_from_the_queued_run` | 取消附带的第二任务，原第一任务回合继续，实际输入只含仍有效的任务 |
| `engine::queued_external_turn_requires_stop_confirmation_for_task_and_run_cancellation` | 带外部 ID 的排队回合，无论取消任务还是回合均保持取消请求，输入未丢弃，不伪报 CANCELLED |
| `recovery::unstarted_member_with_an_unreadable_view_can_be_cancelled_without_a_model_request` | 生产 `serve` 的活动态/暂停态，以及 SIGKILL 后重开旧取消请求；无旧模型请求/新回合，修复字段并重开后新输入正常完成 |
| `recovery::returned_chat_outcome_survives_read_failure_across_close_and_sigkill` | 模型成功/已知失败两类结果，在状态读取和归档受阻时分别正常关闭/SIGKILL；修复后同回合归档原文本或错误，无新模型请求、文件副作用不重做 |

前六项属于 core 集成测试，后两项属于 engine；另扩展已有 `cancel_task_cancels_queued_run`。
结果等待期间退出的四种组合在本批生产修复前已经通过，日志
`/tmp/teamagents-outcome-read-restart-20260919.log`；它是新增证据，不是本批新修复。

专项检查曾将会话总回合数当作被取消成员的回合数；取消通知会合法唤醒 Leader，导致两项探针误报。
最终断言按目标成员限定，继续检查其原回合身份、输入与状态，不将合法通知误算为重复执行。

## 最终验证

```bash
cargo test --offline --locked --manifest-path core/Cargo.toml --test engine --test stored_integrity --test task_boundaries
cargo test --offline --locked --manifest-path engine/Cargo.toml --test recovery --test scenarios --test codex_recovery
make fmt
make check
make pty
```

core 专项 **63 + 14 + 15** 项通过，engine 专项 **18 + 10 + 7** 项通过。
`make check` 的格式、全目标严格 Clippy、回归及仓库卫生检查通过：
**core 145 / engine 358 / TUI 105，共 608 项**，engine 仍有三项显式 ignored。
`make pty` 的输入/恢复、鼠标命中和实际文件审查三项通过。

完整日志：

- `/tmp/teamagents-queued-cancel-core-20260919.log`
- `/tmp/teamagents-queued-cancel-engine-20260919.log`
- `/tmp/teamagents-queued-cancel-check-20260919.log`
- `/tmp/teamagents-queued-cancel-pty-20260919.log`

## 保留边界

- QUEUED 也可能是恢复后的后续片段；“排队期间取消”不表示该回合从未产生副作用。
  已写文件和外部请求保留，没有新增外部 exactly-once 保证或数据库自动修复。
- 运行中取消与已返回结果之间的竞态、等待归档时取消的优先级，以及其他恢复调度故障组合仍需专属检查。
  plain REPL 的拒绝回执文案与持久错误下的等待尚未收口。
- 没有新增真实供应商、远端工具、真实模型 TUI、陌生仓库任务或发行制品验收。
  T1–T24 仍为十八项有路径证据、六项部分覆盖；总体成熟度目标未完成。

后续[plain 与终态恢复批次](repl-finalization-2026-09-19.md)处理 plain 的拒绝回执与存储等待反馈，
并验证已返回成功/失败结果在读取等待、归档重试期间收到取消后的进程内恢复与 SIGKILL 重开。
修复 Chat 已知终态被迟到中断覆盖，以及终态冷恢复重新排队时被旧取消标记丢弃的问题。
尚未返回终态的取消、挂起结果归档与通知时序、其他恢复调度组合仍不在这组专属证据内。
