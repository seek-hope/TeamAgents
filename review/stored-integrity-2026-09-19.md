# 持久业务记录完整性与结算回滚

本批继续方案 §5.2、§6.2、§7、§9 与 D-32/D-35：损坏的业务 JSON 必须明确报错，
不能当作空任务、空输入或缺失批准；存储失败不得提交部分结算。保持 Rust 三 crate、
现有 SQLite 事务和数据库格式，无新增依赖、迁移或架构偏离。

## 修复前证据

最初八项行为回归全部先在原实现上失败，日志：
`/tmp/teamagents-stored-integrity-before-20260919.log`。

- 任务依赖解析失败退回空数组，调度仍成功；回合输入解析失败同样退回空数组，
  后续追加输入可以覆盖损坏的原记录。
- 投递 payload/override 格式错误与权限撤销共用拒绝分支；成员视图成功返回空收件箱，
  损坏投递可被标为 dropped。
- 共享游标查询失败退回零；成员状态和共享读取失败被隐藏为缺失状态或空结果。
- 已完成任务的成果引用解析失败后，等待者仍能启动并拿到空成果。
- 坏批准 scope 被当作 null；坏会话批准缓存被当作没有授权。
- 第二项批准过期注入 SQLite 写故障后，第一项已经过期，外围回合仍结算成功。
  原 `Control::expire_run_approvals` 忽略了 Store 返回的错误。

## 当前行为

1. 任务的 dependencies/result_refs、回合的 input_delivery_ids/waiting_on 按实际列表类型读取；
   事件载荷与受众、批准 scope 和会话批准缓存不再用默认值掩盖解码错误。
   错误指出表/字段、错误类别及位置，不复制 JSON 正文。事件种类使用现有枚举校验。
2. 待投递记录在 Store 读取时检查事件种类、受众字符串列表、对象载荷和可选对象覆盖。
   这样格式错误会中止整个读取事务，原记录保持不变；真正的撤权仍按既有规则留原因并失效。
   原覆盖损坏测试改为断言明确报错、无私有原文泄漏和投递原样保留，没有放宽 ACL。
3. 成员视图向上传播任务、投递、游标、共享条目、成员状态和版本读取错误；
   不再发送由错误默认值拼成的部分视图。视图失败也会回滚本次投递裁剪。
4. 调度依赖、等待任务、活动回合及边界补丁读取失败向上传播；
   `wake_info` 的批准、投递种类和任务结果读取也返回错误。启动中的唤醒读取失败
   回滚回合状态、成员状态及 lifecycle 事件。确实不存在或无权查看的任务仍只返回 UNKNOWN。
5. 批准过期失败向外围事务传播。正常归档、停止确认超时和挂起回合取消三条路径，
   第二个批准写失败都会回滚第一个批准和关联状态；故障解除后同一操作正常完成。
6. 生产 `open_session` 既有的启动预检现在能识别上述坏业务行。
   即使指定新的初始配置或全自动模式，也先拒绝启动，不改原权限、配置、任务、
   回合、投递或审计，不构建成员、不请求模型。修复原字段后可以继续原排队回合。

合法空数组、SQL NULL 的可选投递覆盖、合法 JSON null 的批准 scope 继续可读。
事件的普通审计读取保留合法 JSON payload；实际投递仍要求对象，沿用原投递契约。

## 可复跑检查

```bash
cargo test --offline --locked --manifest-path core/Cargo.toml --test stored_integrity
cargo test --offline --locked --manifest-path core/Cargo.toml --test delivery_acl
cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e \
  persisted_work_is_checked_before_session_start_and_resumes_after_repair
make check
make pty
```

新增十项 core 回归位于 `core/tests/stored_integrity.rs`：

| 范围 | 测试名 |
|---|---|
| 任务和回合列表 | `malformed_task_lists_cannot_skip_dependencies_or_become_empty_assignments`、`malformed_run_lists_cannot_be_overwritten_when_inputs_arrive` |
| 投递及视图 | `malformed_pending_deliveries_are_preserved_instead_of_dropped`、`unreadable_view_metadata_rolls_back_delivery_projection` |
| 批准解码和事务 | `corrupt_approval_scope_prevents_finalization_and_preserves_the_decision`、`approval_expiry_failure_rolls_back_finalization_timeout_and_parked_cancel` |
| 等待结果和依赖 | `corrupt_wake_results_cannot_start_a_turn_with_empty_task_results`、`unreadable_terminal_dependency_cannot_silently_stall_scheduling` |
| 已有批准和缓存 | `unreadable_decided_approval_cannot_start_as_an_unrelated_wake`、`corrupt_session_approval_cache_is_not_reported_as_a_missing_grant` |

检查对 SQLite 原始值做快照，不依赖被测解码器来证明损坏内容保留；同时验证修复后的正常路径。
故障包含截断 JSON、合法 JSON 但类型错误、查询失败及第二次写入失败。

新增一项 engine 回归经生产 `open_session`、Runtime、ChatRunner 和本地 HTTP 模型夹具：
八类坏任务/回合/批准/事件分别尝试未指定配置与显式新配置恢复，均启用启动参数 full_auto，
确认拒绝前没有切换权限或产生模型请求。修复后每类原回合恰好三次请求，
写出准确报告并只提交一次 goal_done；原用户文件及输入保留。
这里的“修复”是测试恢复先前保存的字段字节，不是产品自动修复数据库。

最初八项修复后的定向日志：
`/tmp/teamagents-stored-integrity-targeted-20260919.log`；
生产入口定向日志：`/tmp/teamagents-stored-integrity-engine-20260919.log`。
首次 core 全量检查暴露上述旧覆盖测试仍要求返回空视图，按新的明确错误和保留输入契约更新断言后，
统一 `make check` 通过格式、严格 Clippy 和 **core 137 / engine 349 / TUI 104**，
engine 仍有三项显式 ignored；记录见 `/tmp/teamagents-stored-integrity-check-20260919.log`。
这些 Cargo 数量不等于真实供应商验收数量。

`make pty` 的输入/恢复冒烟、滚动鼠标命中、工作区审查及恢复基线三项全部通过，
日志：`/tmp/teamagents-stored-integrity-pty-20260919.log`。

## 验收边界

- 本批是所列字段和生产路径的错误传播检查，不是全库完整性扫描或自动修复工具。
  `state` 仍只读取活动回合、最近五十个终态回合和当前事件页；旧历史、孤立引用、
  数值语义及其他管理接口不因本批获得完整验证。
- 校验入口部分读取错误仍可能显示为未知引用；它们拒绝动作，但诊断尚未全部统一。
- 没有新增 SIGKILL、真实供应商、远端 MCP、真实模型 TUI 或发行制品证据。
  T1–T24 仍为十八项有路径证据、六项部分覆盖；不据此宣称达到竞品成熟度。
