# 运行中消息投递与存储恢复

本批继续方案 §6.3、§7、§9 与 D-32/D-35，检查已受理的补充消息在存储故障后能否进入原回合。
保持 SQLite 的投递账本与确认规则，不新增持久队列或数据库格式。

## 修复前复现

两项生产路径回归均先在修复前失败，日志：
`/tmp/teamagents-mid-turn-storage-before-20260919.log`。

1. `mid_turn_input_retries_after_a_state_read_failure_without_another_user_action`：
   真实 `serve` 正在等待本地模型响应时，注入旧任务 `result_refs` 读取故障，再提交用户补充。
   输入回执成功，持久投递仍在；旧实现清空 core 的消息缓冲后再读完整状态，读取失败导致未发送。
   修复字段后，原回合的下一次模型请求缺少补充消息。
2. `mid_turn_input_retries_after_a_projection_failure_without_another_user_action`：
   SQLite trigger 在补充消息的成功回执落盘时破坏其投递覆盖字段，使随后的权限投影失败。
   原事务正确保留了缓冲，但运行时没有自动重试；故障解除后若没有其他用户动作或回合结束，
   下一次模型请求仍缺少已受理的消息。

复现只要求在原回合的下一次模型调用前收到已受理补充，不把故障后的额外回合算作通过。
没有调用真实模型供应商。

## 实现

- core 在同一次投递事务中核对回合、接收成员和当前权限，返回带 `run_id/agent_id/items`
  的 `MidTurnPush`，全部成功后才清空缓冲。中途失败回滚权限投影并保留整批缓冲。
  空缓冲直接返回，不为周期检查开启 SQLite 写事务。
- 内部 `drain_mid_turn` RPC 增加接收成员字段。进程内 `CoreClient` 调用同一核心方法并使用
  类型化结果，清空缓冲后不再查询完整状态或解码路由信息。
- 运行时循环主动派送工具动作产生的消息，并按既有一秒间隔重试失败的投影。
  `runtime_errors` 的 `delivery` 阶段显示等待原因；重试成功后清除。
  用户提交与后台循环的交接串行化，避免两个派送线程颠倒核心批次的交接顺序。
- 原始受众、当前授权和既有载荷上限继续生效；Chat/Codex 在真正写入上下文或发送给外部后端前
  仍进行既有的权限复核。进入内存缓冲不等于已消费，归档继续按后端确认过滤投递 ID。

## 新增检查

| 测试 | 证据 |
|---|---|
| `core/tests/delivery_acl.rs::failed_mid_turn_batch_keeps_routing_and_rechecks_permissions_on_retry` | 两个运行中接收者的批次里，后一个回合损坏使前一个载荷降级回滚；完整缓冲保留。修复后按新权限只交付裁剪过的观察消息，撤权消息不发送、不记消费，重复 drain 为空 |
| `recovery::mid_turn_input_retries_after_a_state_read_failure_without_another_user_action` | 旧任务读取故障不丢补充；实际模型请求中原输入和补充各一次，同一个回合结束后无待消费投递 |
| `recovery::mid_turn_input_retries_after_a_projection_failure_without_another_user_action` | 可读状态包含 `delivery` 诊断；解除投影故障后，无新动作也能恢复，后续补充不造成重复注入 |
| `recovery::member_tool_messages_reach_a_running_peer_before_the_sender_finishes` | 两个 Chat 成员同时保持模型请求在途；发送方 `send_message` 后仍未结束，接收方下一次请求已有消息，连续调用只含一份 |
| `recovery::chat_cold_recovery_retries_requeue_failure_without_replaying_tools` | 实际 SIGKILL 后，SQLite 拒绝重新排队，运行时报告 `reconcile` 且不启动模型；解除后同一回合沿用原工具结果，重启间用户改动的文件不被旧写入覆盖 |

成员消息检查初次把模型收到的工具结果误按外层 `Receipt.ok` 断言；实际载荷是
`{"delivered_to":["leader"]}`。核对协议后修正探针，专项及最终全量通过；不将探针错误算作产品缺陷。

## 复跑与验证

```bash
cargo test --offline --locked --manifest-path core/Cargo.toml --test delivery_acl --test engine
cargo test --offline --locked --manifest-path engine/Cargo.toml --test recovery
make check
make pty
```

`make check` 通过格式、全部目标严格 Clippy、回归与仓库卫生检查：
**core 139 / engine 356 / TUI 105**；engine 仍有三项显式 ignored。
`recovery` 的最终 16 项全部通过，现有 Chat/Codex 权限、取消和冷恢复回归继续通过。
完整日志为 `/tmp/teamagents-mid-turn-storage-check-20260919.log`；
core 专项日志为 `/tmp/teamagents-mid-turn-storage-core-20260919.log`；
成员消息专项为 `/tmp/teamagents-mid-turn-tool-message-20260919.log`。

`make pty` 的输入/恢复、滚动鼠标命中、工作区审查三项通过，
日志 `/tmp/teamagents-mid-turn-storage-pty-20260919.log`。

## 范围

- 新增实际 SIGKILL 证据仅覆盖 Chat 重新排队受阻后恢复；成员结果返回后等待状态读取期间的
  退出、取消与准备失败组合、恢复调度阶段故障，仍没有本批专属证明。
- 持久投递和成员检查点仍是冷恢复依据；本批没有数据库自动修复或通用外部 exactly-once 承诺。
  批量投影遇到坏记录时保留整批等待恢复，没有增加坏记录隔离队列。
- 未新增真实供应商、远端工具、真实模型 TUI、陌生仓库或发行制品验收。
  600 项 Cargo 通过不等于发布条件全部满足；T1–T24 仍为十八项有路径证据、六项部分覆盖。

后续[排队取消批次](queued-cancellation-2026-09-19.md)补齐成员视图准备失败时取消、
旧取消请求重开和结果等待状态读取期间正常关闭/实际 SIGKILL 的证据；结果恢复组合在该批修复前已通过。
本报告没有证明的其他恢复调度故障与真实服务边界保持。
