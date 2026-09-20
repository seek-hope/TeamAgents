# 投递时授权复核（2026-09-19）

接续[任务等待隔离](wait-task-isolation-2026-09-19.md)，本批修复 B-03：
消息已经进入持久队列或运行器缓冲，但尚未进入成员上下文时，旧载荷不再绕过当前授权。
依据实施方案 §7 的当前授权复核、§8 的权限变更安全边界与 §9.2 的投递确认要求，没有增加产品范围或依赖。

## 先复现

新增的四项确定性回归在实现修改前全部失败：

- 撤销观察者后，旧 `task_completed` 仍出现在 AgentView，含 `PRIVATE-REF`。
- 撤销消息通道后，目标仍收到旧正文 `PRIVATE-MESSAGE`。
- 内存中的 mid-turn push 在范围降为 `status` 后仍带原摘要与结果。
- `payload_override` 损坏时，视图回退到未裁剪的事件正文。

原始输出保存在本机 `/tmp/teamagents-delivery-acl-before-20260919.log`。这些字符串均为测试哨兵。
随后补充独立订阅、共享空间、事务失败、执行边界及两个真实适配器的线协议检查。

## 最终行为

`core/src/views.rs::project_delivery` 是普通视图、mid-turn 通知与后端注入的共同权限投影：

1. 收件人必须仍在当前团队中，且属于事件原始 `audience`。
2. 直接消息仍需当前通道；`request_help` 的窄返回路径与任务回执保持可用。
   不具备直接访问权时，按当前观察对象、事件类型、载荷范围检查独立观察授权。
3. 共享通知复核当前空间读写权限；观察订阅单独按其范围授权。
4. 投递原有裁剪结果始终是上限；降级后将更窄载荷写回投递行。之后重新授权不扩大这条旧投递，
   已失效投递也不会复活。原始事件与发送时受众不改写。
5. 失效或损坏的投递置 `dropped` 并保留 `dropped_reason`。丢弃不推进已消费批次，
   迟到 ack 不会将 dropped 改成 applied。投影/审计写入失败向上传播，拓扑事务整体回滚。

`Control::agent_view`、调度、拓扑应用和 `drain_mid_turn_pushes` 使用同一投影；
已结束回合不再接收残留 mid-turn push。`payload_scope` 与 `wake_policy` 纳入观察者权限变化判定，
只影响接收观察者；范围变更等待该成员执行边界，不会因仅观察规则改变而阻塞被观察成员。

Chat 在首次输入和缓冲通知真正写入私有历史前，通过持久投递 ID 重新取当前投影。
Codex 缓冲保留结构化投递，`turn/start`（包括 effort 重试）与 `turn/steer` 发送前同样复核；
不再把旧文本直接拼回下一个输入。

同时修正投递确认：runtime 只登记实际提供的 ID；Codex 仅在 app-server 接受输入后，把确认 ID
写入 SQLite 的每回合元数据。运行器重建后仍能读取该记录，发送失败不确认，最终归档以确认为准。
确认迟于回合终结时在同一事务中结清对应 pending 行，避免终态输入留待重放。

## 自动化证据

`core/tests/delivery_acl.rs` 新增九项：

| 测试 | 检查 |
|---|---|
| `queued_observer_deliveries_follow_revocation_and_never_regain_old_payload` | 撤销、降级、改对象、改事件；恢复授权不补旧权限；失效原因、消费批次与原始事件保留 |
| `buffered_mid_turn_push_is_rechecked_before_drain` | 缓冲通知排队后降级，实际 drain 不泄露旧载荷 |
| `queued_direct_message_is_dropped_when_channel_is_revoked` | 经生产拓扑 patch 撤通道，旧消息失效 |
| `invalid_observer_override_never_falls_back_to_the_raw_payload` | 损坏裁剪数据不回退原文 |
| `original_audience_and_payload_remain_limits_after_a_new_grant` | 新授权不突破旧裁剪；伪造旧投递行也不能越过原始受众 |
| `shared_delivery_loses_read_access_but_task_receipts_keep_their_return_path` | 共享读权撤销；正常任务回执与求助不要求通用反向消息通道 |
| `delivery_rejection_failure_rolls_back_the_permission_change` | 注入 SQLite 更新失败，权限版本与投递状态一并回滚 |
| `external_acceptance_ledger_does_not_consume_offers_or_accept_foreign_ids` | 确认与消费分离、跨成员 ID 拒绝、幂等及终态后的迟到确认 |
| `scope_only_patch_waits_for_the_receiving_observers_execution_boundary` | 范围降级进入 WAITING_BOUNDARY，安全边界后旧投递缩减，终态 push 不再发送 |

Engine 新增三项：

- `chat_e2e::stale_views_and_buffered_inbox_are_rechecked_before_chat_history`：
  旧视图/缓冲 × 撤销/降级，检查本地假 HTTP 服务真正收到的模型请求以及检查点的已应用 ID。
- `codex_contract::codex_rechecks_stale_and_queued_input_and_confirms_only_accepted_ids`：
  同样的四种组合检查假 app-server 的 `turn/start` 请求；确认记录在重建运行器后保持。
- `codex_contract::codex_steer_rechecks_scope_and_failed_delivery_remains_pending`：
  实际 `turn/steer` 报文缩减；成功时确认，服务拒绝时归档后仍保留 pending。

复跑：

```bash
cargo test --offline --locked --manifest-path core/Cargo.toml --test delivery_acl
cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e stale_views_and_buffered_inbox_are_rechecked_before_chat_history
cargo test --offline --locked --manifest-path engine/Cargo.toml --test codex_contract
make check
make pty
git diff --check
```

本轮首次 `make check` 在既有 `process_leaks::codex_failed_initialize_reaps_the_child` 遇到
`Text file busy`；同一用例独立重跑通过。该波动在 9 月 14 日、17 日记录中也出现过。
夹具现使用固定 `/bin/sh` 读取工作目录下的 `app-server` 文件，避免直接 exec 刚写入的脚本；
仍走生产 `CodexAppServer::start` 的 initialize 失败路径并断言进程真正回收，没有对生产启动加盲重试。
首次失败日志：`/tmp/teamagents-delivery-acl-check-20260919.log`。

最终 `make check` 通过：格式、全部目标 Clippy（`-D warnings`）、core **78 passed**、
engine **281 passed / 1 ignored**、tui **104 passed**、仓库卫生检查均通过。
`make pty` 的输入/粘贴/退出、鼠标命中和工作区审查三项真终端检查全部通过。
`git diff --check`、本记录的测试名称及本地文档链接检查通过。
最终日志为 `/tmp/teamagents-delivery-acl-check-final-20260919.log`、
`/tmp/teamagents-delivery-acl-pty-20260919.log`。

## 验收边界

B-03 的本地队列、缓冲通知与适配器输入路径已有回归证据。已经写入 Chat 历史或被外部后端接受的内容
不因后续撤权消失；这符合方案定义。`save_spec` 是内部引导接口，运行中权限变更应走拓扑 patch。

本批使用本地假模型/app-server 和真实 SQLite/线程/进程，不等于真实供应商验收。
外部已接受请求但确认尚未落盘时崩溃，仍是分布式确认窗口；本批不宣称跨外部后端的 exactly-once。
T6 的全部私有上下文条件、T17 的真实恢复、多供应商发布矩阵与同条件竞品对照继续保持未完成。
