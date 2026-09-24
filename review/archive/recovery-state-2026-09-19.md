# 归档通知与旧排队回合的恢复一致性

本批延续 D-32/D-35 与方案 §9，处理归档事务结果和通知不一致，以及旧版排队记录在恢复时丢失已有结果。
保持 Rust 三 crate、SQLite 单事务权威与现有数据库格式。使用确定性成员、本地 HTTP 模型夹具、
实际 `serve` 子进程和 SIGKILL，没有调用真实供应商，也没有提交或发布累计修改。

## 已复现的问题

### 1. 挂起归档重试后，通知使用了旧结果

SQLite trigger 阻止 WAITING_TASK / WAITING_APPROVAL 归档，待运行时进入重试后解除故障。
三条独立检查在修复前均失败：

- 等待任务的结果归档前收到取消：同一个核心事务最终将回合置为 CANCELLED，钩子却发出 `run_paused`。
- 批准已处理、挂起结果后到：核心将回合恢复为 RUNNING，钩子仍发出 `run_paused`，之后才发完成。
- 回合已由其他路径结清：核心拒绝覆盖终态，重试是无变更操作，运行时仍发出旧的暂停通知。

第三条探针初版直接结清时遗漏输入确认，另外启动了新回合；修正探针的 `ack_ids` 后，旧暂停通知仍复现。
初版和修正后的日志都保留，结论以修正后的探针为准。

`Control::finalize_run` 现在在同一事务内返回 `FinalizationResult { applied, status }`；
状态取自调度完成后的回合记录，终态无变更返回 `applied=false`。原 JSON 回复保留 `ok`，
添加这两个字段。运行时只按该次提交的状态发通知：无变更不重复发，已恢复执行不误报暂停，
核心转为取消则发取消，不携带旧挂起结果的文本或错误。

这不是提交后的另一次状态读取，也不是新增通知权威。事务之后发生的新动作仍可能再次改变状态，
钩子不是持久、保证送达的事件订阅。

### 2. 旧版 QUEUED + 取消标记覆盖终态检查点

实际成员先写文件，再保存成功回复或明确失败。阻止归档、请求取消并 SIGKILL 后，
把业务记录改为旧版可能留下的 QUEUED 中间态；检查点保留原样。修复前重开得到 CANCELLED，
成功回复未归档。此前只覆盖了业务记录仍为 RUNNING 的冷恢复。

恢复现在先检查排队回合是否有成员检查点或外部回合 ID。已有执行证据的整批回合在一个核心事务中
恢复为 RUNNING，再逐个核对后端结果；保留取消标记、输入、任务及原回合 ID，不重新记账开始事件。
批量准备失败整体回滚并重试。没有执行证据的新意图继续排队，损坏检查点进入已有的结果不明处理。
这个内部方法由进程内客户端调用，没有增加面向用户的任意状态写入接口。

必须先准备整批：归档第一个结果、或者 Codex 恢复时补交完成申请，都会调度其他成员。
只在处理到某个回合时才保护它，会提前取消尚未核对的其他旧回合。

另外实测了 `exec --resume` 的入口顺序：它先提交新输入，再启动运行时。只修复 `start` 的恢复
仍会丢失旧结果；生产调用顺序的回归再次复现 CANCELLED。现在首次提交及恢复等待期间的提交，
都先完成恢复准备。存储仍阻止准备时，本次输入明确失败；排除故障后原工作仍能恢复。
重开时开启全自动也先准备旧回合，再提交模式变更。

## 检查范围

新增三项 core 回归：

| 检查 | 证明范围 |
|---|---|
| `task_boundaries::finalization_reports_the_committed_status_and_preserves_settled_results` | 等待后取消、批准已处理、成功、失败的实际提交状态；重复归档不覆盖终态或追加事件 |
| `stored_integrity::queued_recovery_admission_rolls_back_the_entire_batch_on_storage_failure` | 第二个回合写入失败使整批回滚；修复后保留取消和输入，保留 draining 状态；重复准备无变更 |
| `task_boundaries::queued_recovery_admission_cannot_cross_sessions_or_revive_a_settled_run` | 外会话引用使整批拒绝；已结清回合不复活 |

新增六项 engine 回归，均在 `engine/tests/recovery.rs`：

| 检查 | 证明范围 |
|---|---|
| `finalization_hooks_report_a_cancellation_committed_while_parking` | 重试归档后只发一次取消通知，状态与该次提交一致 |
| `finalization_hooks_do_not_repeat_a_superseded_outcome` | 旧结果被终态保护拒绝后不发过时通知 |
| `finalization_hooks_do_not_report_an_already_resolved_approval_as_paused` | 批准挂起已被核心恢复时不发暂停，正常续跑并完成 |
| `returned_chat_outcome_survives_a_legacy_queued_cancellation` | 成功/明确失败 × 普通重开/开启全自动/恢复写入故障后重试；实际 SIGKILL、原回复或错误保留，没有追加模型请求，文件内容保持一致；恢复受阻时新输入拒绝 |
| `queued_chat_recovery_distinguishes_unstarted_and_invalid_checkpoints` | 实际 SIGKILL 后，新意图只执行原输入一次；存在损坏检查点且带取消标记时保留文件并进入 OUTCOME_UNKNOWN，不请求模型 |
| `queued_recovery_admits_all_saved_runs_before_any_result_can_schedule_peers` | 两个旧回合都被恢复，完成申请不会提前取消同伴；覆盖直接启动以及 CLI 的先提交输入再启动顺序 |

沿用原归档失败、取消、历史提交日志、Chat/Codex 冷恢复和会话启动检查。

## 验证

```bash
cargo test --offline --locked --manifest-path core/Cargo.toml --test task_boundaries --test stored_integrity
cargo test --offline --locked --manifest-path engine/Cargo.toml --test recovery --test chat_e2e --test codex_recovery --test session_boot
make fmt
make check
make pty
bash review/eval/check-runner.sh
```

完整 `make check` 的格式、全目标严格 Clippy、回归及仓库卫生检查通过：
**core 148 / engine 368 / TUI 105，共 621 项**；engine 仍有三项显式 ignored。
其中 `recovery` 28 项、`chat_e2e` 59 项、`codex_recovery` 7 项、`session_boot` 3 项通过。
三项 PTY、runner 30 项无模型契约通过，本机 bubblewrap 隔离探针成功。
八份相关文档的 122 项本地链接有效，`git diff --check` 通过。没有新增真实模型或发行成绩。

复现与专项日志：

- `/tmp/teamagents-finalization-hooks-before-20260919.log`
- `/tmp/teamagents-finalization-hooks-before2-20260919.log`
- `/tmp/teamagents-finalization-hooks-fixed-20260919.log`
- `/tmp/teamagents-legacy-queued-before-20260919.log`
- `/tmp/teamagents-legacy-queued-fixed-20260919.log`
- `/tmp/teamagents-prestart-recovery-before-20260919.log`
- `/tmp/teamagents-recovery-state-core-20260919.log`
- `/tmp/teamagents-recovery-state-focused-20260919.log`
- `/tmp/teamagents-recovery-state-check-20260919.log`
- `/tmp/teamagents-recovery-state-pty-20260919.log`
- `/tmp/teamagents-recovery-state-runner-20260919.log`

## 保留边界

- 恢复识别依据是本地检查点或外部回合 ID。旧排队记录若同时丢失这些执行证据，本批不能证明其
  与从未开始的意图可被区分；尚未增加基于历史开始事件的回查。
  后续[恢复证据批次](recovery-evidence-2026-09-19.md)已补充数据库开始事件回查及清理保护，
  全部执行依据同时丢失时的限制仍保留。
- 两成员互相影响使用确定性后端；SIGKILL 及模式切换使用真实进程、本地 HTTP 协议夹具，
  不算真实供应商或完整混队验收。外部副作用未知时仍不作通用 exactly-once 承诺。
- 没有新增陌生仓库成绩、真实远端工具、真实模型 TUI 或发行制品验收。
  T1–T24 仍为十八项有路径证据、六项部分覆盖；总体成熟度目标保持进行中。
