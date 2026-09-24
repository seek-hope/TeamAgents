# Chat 冷恢复与回合归档（2026-09-19）

按方案 §9.2/§9.3 与 D-32/D-35，补充真实进程崩溃窗口验收并修复两处重复执行风险。
本批没有调用真实供应商服务；模型为可控本地 HTTP 夹具。完整日志和本批增量补丁见
[归档](eval/runs/2026-09-19-chat-cold-recovery/REPORT.md)。

## 先验证既有路径

三项新测试直接启动 `teamagents serve`，不传 `scripts`，使用生产 ChatRunner、
真实 SQLite 与 bubblewrap Shell，并以 `Child::kill` 后的 signal 9 确认真正 SIGKILL：

- Shell 结果已入检查点、下一模型请求尚未返回时崩溃；恢复保留原工具结果，文件追加只有一行。
  崩溃前收到但尚未写入检查点的用户补充在重启请求里出现一次；原用户输入也只有一次。
- WAITING_APPROVAL 后崩溃；重开保留同一个批准，批准前无模型请求、无文件副作用。
  用户批准 once 后模型重发同操作，执行一次且批准置 EXPIRED。
- Shell 已追加文件、尚在 sleep、结果未保存时崩溃；重开进入 OUTCOME_UNKNOWN，
  不调用模型，不重做副作用，原检查点逐字节保持。

这三项在修改生产代码前均通过。首次测试错误地期望成功工具内容带 `ok` 字段；
实际成功格式为结果对象，修正为检查结果/无 error 后才取得有效基线，不将该错误算作产品缺陷。

## 两处失败与修复

**F-1：核心归档失败使成员被重新启动。** 在 Shell 成功后让本地模型返回 HTTP 503，
以 SQLite trigger 拒绝把回合置 FAILED。旧 `Runtime::finalize` 只记日志并发送失败通知，
回合仍处于 RUNNING，成员包装线程退出后调度器再次启动同一回合，出现额外模型请求。
`finalization-red.log` 在“不得有额外请求”的断言失败。

Runtime 现在保留原 TurnRun、TurnOutcome 与本次投递确认，只重试 `finalize_run` 事务；
同一 run_id 在等待归档期间不再调度。失败后间隔一秒重试，相同错误不重复刷日志。
成功后才发送终态通知；暂停会话也继续重试，其他成员仍可执行。
该队列只在当前进程内保留，不是第二份业务权威；冷恢复继续依赖成员持久证据。

**F-2：模型失败未保存，归档失败加崩溃后仍会重问。** 仅修 Runtime 后，让 HTTP 503 的
失败结果归档受阻，再 SIGKILL、移除故障并重开，仍产生新模型请求。
`failed-cold-red.log` 复现。原因是 `ChatError` 原先直接从 run_segment 返回，没有保存终态。

Chat 对已知模型/协议失败写 FAILED 检查点并完成已有树提交流程，再把结果交给 Runtime。
恢复沿同一 run_id 读取该终态，归档原错误，不重复请求。
被中断、检查点损坏或外部结果未知仍沿原拒绝路径，不为记录失败而覆盖恢复证据。

## 回归范围

新增 **7 项**，`recovery` 二进制共 **9 项**：

| 测试 | 行为 |
|---|---|
| `chat_cold_resume_keeps_tool_results_and_unconsumed_supplements` | 工具结果、原输入和迟到补充跨真实 SIGKILL 恢复；文件副作用一次 |
| `chat_cold_resume_preserves_pending_approval_and_consumes_it_once` | 待批准跨 SIGKILL；授权前无副作用，授权后一次并消费 |
| `chat_cold_resume_does_not_repeat_an_external_effect_without_a_receipt` | 无结果外部操作进入未知，检查点与已有副作用保留 |
| `chat_failed_outcome_commit_is_retried_without_restarting_the_member` | SQL 故障解除后只归档原模型错误；失败事件一次、Shell 一次、无额外模型请求 |
| `chat_completed_checkpoint_survives_an_uncommitted_outcome_and_shutdown` | 普通关闭和 SIGKILL 两种方式均从已完成检查点补归档，最终回复只出现一次 |
| `chat_model_failure_survives_a_failed_commit_and_sigkill_without_another_request` | HTTP 503 已知失败先持久化，归档失败再崩溃仍不重问 |
| `finalization_retries_while_paused_without_blocking_other_members_or_premature_hooks` | 暂停时重试归档、保留未执行通知；其他成员先完成，hooks 仅提交后收到一次终态，原投递确认保留 |

最后一项是进程内脚本成员与真实 SQLite 故障检查；其余六项走生产 serve/Chat。
需要 Shell 的四项在无 bubblewrap 时显式打印 skip 后返回，本机本批均实际执行。
HTTP 夹具不验证某个真实供应商的行为。

```bash
cargo test --offline --locked --manifest-path engine/Cargo.toml --test recovery -- --nocapture
make check
```

最终定向 **9/9**；`make check` 退出 **0**，耗时 **77.852 秒**：
core **90**、engine **329**（108 库 + 1 CLI + 220 集成；另 3 ignored）、TUI **104**。
格式、全部目标严格 Clippy 和仓库卫生通过；本批无 TUI 布局/操作变化，未重复 PTY。

暂停回归的开发中曾误用 `settle` 要求队列为空；诊断证明失败结果已归档，队列里是暂停后应保留的
新任务通知。改为核对原回合失败、无 RUNNING、通知仍 QUEUED，不把探针误差描述为产品缺陷。

## 保证边界

故障注入用可解除的 SQLite trigger，证明事务失败重试与结果保留，不代表磁盘损坏/断电验收。
进程内等待归档表退出后不保留，生产冷恢复仍使用 Chat 检查点或 Codex 外部证据。
没有通用外部 exactly-once，没有自动修复损坏库；持续存储故障仍会阻止归档。
终态 hooks 以提交成功为前提，但不是跨崩溃的持久通知队列。
本批未增加真实供应商、真实大仓库完成率或竞品成绩，长期目标仍在进行中。
